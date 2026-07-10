# wl-showkeys

Displays keypresses on screen on supported Wayland compositors (requires `wlr_layer_shell_v1` support).

Forked from <https://git.sr.ht/~sircmpwn/wshowkeys> as Drew has moved onto other things.

Then forked again from <https://github.com/ammgws/wshowkeys> to enhance character replacements.

Then migrated to Rust by benjilegnard (and claude).

Then renamed to `wl-showkeys` and enhanced.

## Installation

These dependencies should be available on your linux system

- cairo
- libinput
- pango
- udev 
- wayland 
- xkbcommon 

## Compilation

```
cargo build --release
sudo chown root target/release/wl-showkeys
sudo chmod u+s target/release/wl-showkeys
cp ./target/release/wl-showkeys ~/.local/bin/
```

`wl-showkeys` must be configured as setuid during installation.
It requires root permissions to read input events.
These permissions are dropped after startup.

## Usage

```
wl-showkeys [-b|-f|-s #RRGGBB[AA]] [-F font] [-t timeout]
    [-a top|left|right|bottom] [-m margin] [-o output]
```

- *-b #RRGGBB[AA]*: set background color
- *-f #RRGGBB[AA]*: set foreground color
- *-s #RRGGBB[AA]*: set color for special keys
- *-F font*: set font (Pango format, e.g. 'monospace 24')
- *-t timeout*: set timeout before clearing old keystrokes
- *-a top|left|right|bottom*: anchor the keystrokes to an edge. May be specified
  twice.
- *-m margin*: set a margin (in pixels) from the nearest edge
- *-o output*: request wl-showkeys is shown on the specified output (i.e. `-o HDMI-A-1`)

## Roadmap / Todolist

- [x] add the ability to chose output and draw on specified screen
- [ ] add the ability to configure it with a `~/.config/showkeys/config.toml` file
- [ ] args should be handled better and override config
- [ ] more configuration options (spacing ?, color?)
- [ ] handle special characters better
- [ ] automatic install script / distro packages ?

