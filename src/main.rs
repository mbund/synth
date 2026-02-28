use std::{io::Write, time::{Duration, Instant}};
use ratatui::{Terminal, widgets::Paragraph};
use terminal_games_sdk::{
    app,
    terminal::{TerminalGamesBackend, TerminalReader},
    terminput,
};

#[used]
static TERMINAL_GAMES_MANIFEST: &[u8] = include_bytes!("../terminal-games.json");

const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / 60);

fn main() -> std::io::Result<()> {
    let mut terminal = Terminal::new(TerminalGamesBackend::new(std::io::stdout()))?;
    terminal.clear()?;
    std::io::stdout().write(b"\x1b[?1003h")?;

    let mut terminal_reader = TerminalReader {};
    let start = Instant::now();
    let mut frame_counter = 1;
    let mut last_event = None;
    let mut next_frame = Instant::now();

    'outer: loop {
        if app::graceful_shutdown_poll() {
            break;
        }

        let mut event_counter = 0;
        for event in &mut terminal_reader {
            event_counter += 1;
            if let Some(key_event) = event.as_key() {
                match key_event {
                    terminput::key!(terminput::KeyCode::Char('q')) => break 'outer,
                    _ => {}
                }
            }
            last_event = Some(event);
        }

        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(
                Paragraph::new(format!(
                    "Hello World!\ncounter={}\nlast_event={:#?}\nevent_counter={}\nuptime_secs={:.2}\n",
                    frame_counter,
                    last_event,
                    event_counter,
                    start.elapsed().as_secs_f64(),
                )),
                area,
            );
        })?;

        frame_counter += 1;

        next_frame += FRAME_DURATION;
        if let Some(remaining) = next_frame.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        }
    }

    std::io::stdout().write(b"\x1b[?1003l")?;
    Ok(())
}
