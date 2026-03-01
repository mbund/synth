# synth

From https://musescore.org/en/handbook/3/soundfonts-and-sfz-files "The free default soundfont that comes with MuseScore 1" and licensed under the GNU GPL, version 2.

Run it with the following commands:

```
cargo build --target wasm32-wasip1 --release
```

```
terminal-games-cli ./target/wasm32-wasip1/release/rust-ratatui-template.wasm
```

Download the CLI from the main [Terminal Games repository](https://github.com/terminal-games/terminal-games)
