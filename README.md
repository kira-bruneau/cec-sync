# cec-sync

A service for integrating Linux devices into a home theatre system
over HDMI-CEC

```text
Usage: cec-sync [COMMAND]

Commands:
  serve   Run the cec-sync service [default]
  active  Change active source device
  power   Change device power status
  volume  Change TV / AVR volume
  mute    Change TV / AVR mute status
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

## Implemented backends

### Unix Socket

Used by the CLI when there's a cec-sync server running (started by `cec-sync serve`)

### D-Bus

- **[systemd-logind](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html):**
  Syncs systemd sleep state with the sleep state of your TV

### Wayland

- **[gamescope-input-method](https://github.com/ValveSoftware/gamescope/blob/master/protocol/gamescope-input-method.xml):**
  Handles TV remote input for your Steam Deck

## Planned backends

### D-Bus

- **[MPRIS](https://specifications.freedesktop.org/mpris-spec/latest):**
  Syncs media playback state between your Linux device and your home
  theatre system

### Wayland

- **[virtual-keyboard-unstable-v1](https://wayland.app/protocols/virtual-keyboard-unstable-v1):**
  Handles TV remote input for your Wayland-based desktop environment

### X11

- **[xtest](https://www.x.org/releases/X11R7.7/doc/libXtst/xtestlib.html):**
  Handles TV remote input for your X-based desktop environment
