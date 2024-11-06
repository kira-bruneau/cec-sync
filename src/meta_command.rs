use {
    crate::CecError,
    blocking::unblock,
    cec_rs::{
        CecAudioStatusError, CecConnection, CecDeckInfo, CecDeviceType, CecLogicalAddress,
        CecPowerStatus, CecUserControlCode, KnownAndRegisteredCecLogicalAddress,
    },
    clap::Subcommand,
    postcard::experimental::max_size::MaxSize,
    serde::{Deserialize, Serialize},
    std::{future::Future, sync::Arc},
};

#[derive(Subcommand, Serialize, Deserialize, MaxSize, Debug, Copy, Clone)]
pub enum MetaCommand {
    #[command(subcommand, about = "Change active source device")]
    Active(Active),

    #[command(subcommand, about = "Change device power status")]
    Power(Power),

    #[command(subcommand, about = "Change TV / AVR volume")]
    Volume(Volume),

    #[command(about = "Change TV / AVR mute status")]
    Mute {
        #[command(subcommand)]
        command: Option<Mute>,
    },

    #[clap(skip)]
    DeckInfo(DeckInfo),
}

#[derive(Subcommand, Serialize, Deserialize, MaxSize, Debug, Copy, Clone)]
pub enum Active {
    #[command(about = "Set this device as the active source")]
    Set {
        #[arg(short, long, help = "Only if there's no other active sources")]
        cooperative: bool,
    },

    #[command(about = "Unset this device as the active source")]
    Unset,
}

#[derive(Subcommand, Serialize, Deserialize, MaxSize, Debug, Copy, Clone)]
pub enum Power {
    #[command(about = "Power on all devices")]
    On,

    #[command(about = "Power off all devices")]
    Off {
        #[arg(short, long, help = "Only if this device is the active source")]
        cooperative: bool,
    },
}

#[derive(Subcommand, Serialize, Deserialize, MaxSize, Debug, Copy, Clone)]
pub enum Volume {
    #[command(about = "Increase volume")]
    Up {
        #[arg(default_value_t = 1)]
        steps: u8,
    },

    #[command(about = "Decrease volume")]
    Down {
        #[arg(default_value_t = 1)]
        steps: u8,
    },

    #[command(about = "Set volume (most TVs don't support this)")]
    Set { volume: u8 },
}

#[derive(Subcommand, Serialize, Deserialize, MaxSize, Debug, Copy, Clone)]
pub enum Mute {
    #[command(about = "Toggle TV / AVR mute status [default]")]
    Toggle,

    #[command(about = "Mute TV / AVR (most TVs don't support this)")]
    On,

    #[command(about = "Unmute TV / AVR (most TVs don't support this)")]
    Off,
}

#[derive(
    Subcommand, Serialize, Deserialize, MaxSize, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord,
)]
pub enum DeckInfo {
    Play,
    Still,
    Stop,
}

impl Default for DeckInfo {
    fn default() -> Self {
        DeckInfo::Stop
    }
}

impl From<DeckInfo> for CecDeckInfo {
    fn from(value: DeckInfo) -> Self {
        match value {
            DeckInfo::Play => CecDeckInfo::Play,
            DeckInfo::Still => CecDeckInfo::Still,
            DeckInfo::Stop => CecDeckInfo::Stop,
        }
    }
}

impl MetaCommand {
    pub fn run(self, cec: Arc<CecConnection>) -> impl Future<Output = Result<(), CecError>> {
        unblock(move || self.run_sync(&cec))
    }

    fn run_sync(self, cec: &CecConnection) -> Result<(), CecError> {
        match self {
            MetaCommand::Active(Active::Set { cooperative: false }) => active_set(cec),
            MetaCommand::Active(Active::Set { cooperative: true }) => active_set_cooperative(cec),
            MetaCommand::Active(Active::Unset) => active_unset(cec),
            MetaCommand::Power(Power::On) => power_on(cec),
            MetaCommand::Power(Power::Off { cooperative: false }) => power_off(cec),
            MetaCommand::Power(Power::Off { cooperative: true }) => power_off_cooperative(cec),
            MetaCommand::Volume(Volume::Up { steps }) => volume_up(cec, steps),
            MetaCommand::Volume(Volume::Down { steps }) => volume_down(cec, steps),
            MetaCommand::Volume(Volume::Set { volume }) => volume_set(cec, volume),
            MetaCommand::Mute {
                command: None | Some(Mute::Toggle),
            } => mute_toggle(cec),
            MetaCommand::Mute {
                command: Some(Mute::On),
            } => mute_on(cec),
            MetaCommand::Mute {
                command: Some(Mute::Off),
            } => mute_off(cec),
            MetaCommand::DeckInfo(deck_info) => deck_info_set(cec, deck_info.into()),
        }
    }
}

fn active_set(cec: &CecConnection) -> Result<(), CecError> {
    cec.set_active_source(CecDeviceType::Reserved)?;
    Ok(())
}

fn active_set_cooperative(cec: &CecConnection) -> Result<(), CecError> {
    match cec.get_device_power_status(cec.get_active_source()) {
        CecPowerStatus::InTransitionOnToStandby
        | CecPowerStatus::Standby
        | CecPowerStatus::Unknown => active_set(cec)?,
        CecPowerStatus::InTransitionStandbyToOn | CecPowerStatus::On => (),
    };

    Ok(())
}

fn active_unset(cec: &CecConnection) -> Result<(), CecError> {
    cec.set_inactive_view()?;
    Ok(())
}

fn power_on(cec: &CecConnection) -> Result<(), CecError> {
    cec.send_power_on_devices(CecLogicalAddress::Unregistered)?;
    Ok(())
}

fn power_off(cec: &CecConnection) -> Result<(), CecError> {
    cec.send_standby_devices(CecLogicalAddress::Unregistered)?;
    Ok(())
}

fn power_off_cooperative(cec: &CecConnection) -> Result<(), CecError> {
    let Some(active_source) = KnownAndRegisteredCecLogicalAddress::new(cec.get_active_source())
    else {
        return Ok(());
    };

    // This would only fail if there was a bug in cec-rs or libcec
    let my_addresses = cec.get_logical_addresses().unwrap().addresses;
    if my_addresses.contains(&active_source) {
        power_off(cec)?
    }

    Ok(())
}

fn volume_up(cec: &CecConnection, steps: u8) -> Result<(), CecError> {
    for _ in 0..steps {
        match cec.volume_up(true) {
            Ok(_) => (),
            Err(CecAudioStatusError::Unknown) => (),
            Err(err) => return Err(CecError::AudioStatus(err)),
        }
    }

    Ok(())
}

fn volume_down(cec: &CecConnection, steps: u8) -> Result<(), CecError> {
    for _ in 0..steps {
        match cec.volume_down(true) {
            Ok(_) => (),
            Err(CecAudioStatusError::Unknown) => (),
            Err(err) => return Err(CecError::AudioStatus(err)),
        }
    }

    Ok(())
}

fn volume_set(cec: &CecConnection, volume: u8) -> Result<(), CecError> {
    let status = cec.audio_get_status()?;
    let steps = volume as i8 - status.volume() as i8;
    if steps >= 0 {
        volume_up(cec, steps as u8)?;
    } else {
        volume_down(cec, steps as u8)?;
    }

    Ok(())
}

fn mute_toggle(cec: &CecConnection) -> Result<(), CecError> {
    match cec.audio_toggle_mute() {
        Ok(_) => (),
        Err(CecAudioStatusError::Unknown) => {
            cec.send_keypress(CecLogicalAddress::Tv, CecUserControlCode::Mute, true)?;
            cec.send_key_release(CecLogicalAddress::Tv, true)?
        }
        Err(err) => return Err(CecError::AudioStatus(err)),
    }

    Ok(())
}

fn mute_on(cec: &CecConnection) -> Result<(), CecError> {
    match cec.audio_mute() {
        Ok(_) => (),
        Err(CecAudioStatusError::Unknown) => {
            cec.send_keypress(
                CecLogicalAddress::Tv,
                CecUserControlCode::MuteFunction,
                true,
            )?;
            cec.send_key_release(CecLogicalAddress::Tv, true)?
        }
        Err(err) => return Err(CecError::AudioStatus(err)),
    }

    Ok(())
}

fn mute_off(cec: &CecConnection) -> Result<(), CecError> {
    match cec.audio_mute() {
        Ok(_) => (),
        Err(CecAudioStatusError::Unknown) => {
            cec.send_keypress(
                CecLogicalAddress::Tv,
                CecUserControlCode::RestoreVolumeFunction,
                true,
            )?;
            cec.send_key_release(CecLogicalAddress::Tv, true)?
        }
        Err(err) => return Err(CecError::AudioStatus(err)),
    }

    Ok(())
}

fn deck_info_set(cec: &CecConnection, info: CecDeckInfo) -> Result<(), CecError> {
    cec.set_deck_info(info, true)?;
    Ok(())
}
