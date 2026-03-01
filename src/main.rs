use ratatui::{
    Terminal,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::{
    collections::{HashMap, HashSet},
    env,
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};
use terminal_games_sdk::{
    app,
    audio::{self, AudioWriter, mixer},
    terminal::{TerminalGamesBackend, TerminalReader},
    terminput,
};

#[used]
static TERMINAL_GAMES_MANIFEST: &[u8] = include_bytes!("../terminal-games.json");

static SOUNDFONT: &[u8] = include_bytes!("../TimGM6mb.sf2");

const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / 60);
const MIXER_TICK_INTERVAL: Duration = Duration::from_millis(5);
const LEGACY_NOTE_DURATION: Duration = Duration::from_millis(180);
const SUSTAIN_TAIL_DURATION: Duration = Duration::from_millis(1800);
const ENHANCED_STUCK_TIMEOUT: Duration = Duration::from_millis(1200);
const KITTY_KEYBOARD_ENABLE: &[u8] = b"\x1b[>11u";
const KITTY_KEYBOARD_DISABLE: &[u8] = b"\x1b[<u";
const MOUSE_ENABLE: &[u8] = b"\x1b[?1002h\x1b[?1006h";
const MOUSE_DISABLE: &[u8] = b"\x1b[?1002l\x1b[?1006l";
const MIDI_LOW_NOTE: i32 = 0;
const MIDI_HIGH_NOTE: i32 = 127;
const PIANO_KEY_WIDTH: usize = 4;
const PIANO_HEIGHT: usize = 13;
const SYNTH_CHANNEL: i32 = 0;
const KEYBOARD_BASE_NOTE: i32 = 60;
const KEYBOARD_MAX_SEMITONE: i32 = 13;
const HELP_HEIGHT: usize = 4;
const PICKER_MAX_VISIBLE: usize = 8;
const MIN_CONTENT_WIDTH_FOR_PADDING: usize = 24;
const MIN_CONTENT_HEIGHT_FOR_PADDING: usize = PIANO_HEIGHT + HELP_HEIGHT + 3;
const MAX_UI_COLUMN_WIDTH: usize = 96;

#[derive(Clone)]
struct PresetChoice {
    name: String,
    bank: i32,
    patch: i32,
}

fn apply_preset(synthesizer: &mut rustysynth::Synthesizer, preset: &PresetChoice) {
    let bank = preset.bank.clamp(0, 127);
    let patch = preset.patch.clamp(0, 127);
    synthesizer.process_midi_message(SYNTH_CHANNEL, 0xB0, 0x00, bank);
    synthesizer.process_midi_message(SYNTH_CHANNEL, 0xC0, patch, 0);
}

fn terminal_likely_supports_kitty_protocol() -> bool {
    let term = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
    if ["kitty", "alacritty", "foot", "ghostty", "wezterm", "rio"]
        .iter()
        .any(|name| term.contains(name))
    {
        return true;
    }
    return false;
}

fn key_to_semitone(c: char) -> Option<i32> {
    let c = c.to_ascii_lowercase();
    match c {
        's' => Some(0),
        'd' => Some(2),
        'f' => Some(4),
        'g' => Some(5),
        'h' => Some(7),
        'j' => Some(9),
        'k' => Some(11),
        'l' => Some(12),
        'q' => Some(1),
        'w' => Some(3),
        'e' => Some(6),
        'r' => Some(8),
        't' => Some(10),
        'y' => Some(13),
        _ => None,
    }
}

fn octave_shift_bounds() -> (i32, i32) {
    let min = (MIDI_LOW_NOTE - KEYBOARD_BASE_NOTE).div_euclid(12);
    let max = (MIDI_HIGH_NOTE - KEYBOARD_BASE_NOTE - KEYBOARD_MAX_SEMITONE).div_euclid(12);
    (min, max)
}

fn key_to_note(c: char, keyboard_base_note: i32) -> Option<i32> {
    let semitone = key_to_semitone(c)?;
    let note = keyboard_base_note + semitone;
    (MIDI_LOW_NOTE..=MIDI_HIGH_NOTE)
        .contains(&note)
        .then_some(note)
}

fn display_label_for_note(note: i32, keyboard_base_note: i32) -> char {
    let offset = note - keyboard_base_note;
    match offset {
        0 => 's',
        2 => 'd',
        4 => 'f',
        5 => 'g',
        7 => 'h',
        9 => 'j',
        11 => 'k',
        12 => 'l',
        _ => ' ',
    }
}

fn is_white_key(note: i32) -> bool {
    matches!(note.rem_euclid(12), 0 | 2 | 4 | 5 | 7 | 9 | 11)
}

fn black_after_white(note: i32) -> Option<i32> {
    match note.rem_euclid(12) {
        0 | 2 | 5 | 7 | 9 => Some(note + 1),
        _ => None,
    }
}

fn row_to_line(chars: &[char], styles: &[Style]) -> Line<'static> {
    let mut spans = Vec::new();
    if chars.is_empty() {
        return Line::from("");
    }

    let mut current_style = styles[0];
    let mut current = String::new();
    current.push(chars[0]);

    for i in 1..chars.len() {
        if styles[i] == current_style {
            current.push(chars[i]);
        } else {
            spans.push(Span::styled(current.clone(), current_style));
            current.clear();
            current.push(chars[i]);
            current_style = styles[i];
        }
    }
    spans.push(Span::styled(current, current_style));
    Line::from(spans)
}

fn white_notes() -> Vec<i32> {
    (MIDI_LOW_NOTE..=MIDI_HIGH_NOTE)
        .filter(|n| is_white_key(*n))
        .collect()
}

fn visible_white_count(area_width: usize, total_white_keys: usize) -> usize {
    (area_width.saturating_sub(1) / PIANO_KEY_WIDTH)
        .max(1)
        .min(total_white_keys.max(1))
}

fn piano_width(visible_white_count: usize) -> usize {
    visible_white_count * PIANO_KEY_WIDTH + 1
}

fn piano_left_offset(
    content_width: usize,
    visible_white_count: usize,
    total_white_count: usize,
) -> usize {
    let width = piano_width(visible_white_count);
    if visible_white_count == total_white_count && content_width > width {
        (content_width - width) / 2
    } else {
        0
    }
}

fn should_use_outer_padding(width: usize, height: usize) -> bool {
    width >= MIN_CONTENT_WIDTH_FOR_PADDING + 2 && height >= MIN_CONTENT_HEIGHT_FOR_PADDING + 2
}

fn content_viewport(width: u16, height: u16) -> (usize, usize, usize, usize) {
    let pad = should_use_outer_padding(width as usize, height as usize) as usize;
    (
        pad,
        pad,
        (width as usize).saturating_sub(pad * 2),
        (height as usize).saturating_sub(pad * 2),
    )
}

fn content_area(area: Rect) -> Rect {
    let (x, y, width, height) = content_viewport(area.width, area.height);
    if x > 0 {
        Rect {
            x: area.x + x as u16,
            y: area.y + y as u16,
            width: width as u16,
            height: height as u16,
        }
    } else {
        area
    }
}

fn centered_column(container_width: usize, max_width: usize) -> (usize, usize) {
    let width = container_width.min(max_width).max(1);
    ((container_width.saturating_sub(width)) / 2, width)
}

fn white_index_for_note(all_white_notes: &[i32], note: i32) -> usize {
    let target = if is_white_key(note) { note } else { note - 1 };
    match all_white_notes.binary_search(&target) {
        Ok(idx) => idx,
        Err(idx) => idx
            .saturating_sub(1)
            .min(all_white_notes.len().saturating_sub(1)),
    }
}

fn piano_scroll_with_playable_keys_visible(
    all_white_notes: &[i32],
    piano_scroll: usize,
    white_count: usize,
    keyboard_base_note: i32,
    focus_note: Option<i32>,
) -> usize {
    if all_white_notes.is_empty() || white_count == 0 {
        return 0;
    }

    let max_scroll = all_white_notes.len().saturating_sub(white_count);
    let mut scroll = if let Some(note) = focus_note {
        let focus_idx =
            white_index_for_note(all_white_notes, note.clamp(MIDI_LOW_NOTE, MIDI_HIGH_NOTE));
        focus_idx.saturating_sub(white_count / 2).min(max_scroll)
    } else {
        piano_scroll.min(max_scroll)
    };
    let low_note = keyboard_base_note.clamp(MIDI_LOW_NOTE, MIDI_HIGH_NOTE);
    let high_note =
        (keyboard_base_note + KEYBOARD_MAX_SEMITONE).clamp(MIDI_LOW_NOTE, MIDI_HIGH_NOTE);
    let required_start = white_index_for_note(all_white_notes, low_note);
    let required_end = white_index_for_note(all_white_notes, high_note);
    let required_span = required_end.saturating_sub(required_start) + 1;

    if required_span >= white_count {
        return required_start.min(max_scroll);
    }
    if required_start < scroll {
        scroll = required_start;
    }
    let visible_end = scroll + white_count - 1;
    if required_end > visible_end {
        scroll = required_end + 1 - white_count;
    }
    scroll.min(max_scroll)
}

fn filter_preset_indices(presets: &[PresetChoice], filter: &str) -> Vec<usize> {
    let needle = filter.trim().to_ascii_lowercase();
    presets
        .iter()
        .enumerate()
        .filter_map(|(idx, preset)| {
            let haystack =
                format!("{} {} {}", preset.name, preset.bank, preset.patch).to_ascii_lowercase();
            haystack.contains(&needle).then_some(idx)
        })
        .collect()
}

fn clear_playing_notes(
    synthesizer: &mut rustysynth::Synthesizer,
    active_key_notes: &mut HashMap<char, i32>,
    active_notes: &mut HashSet<i32>,
    legacy_note_off_deadlines: &mut HashMap<i32, Instant>,
    sustained_note_off_deadlines: &mut HashMap<i32, Instant>,
    key_last_seen: &mut HashMap<char, Instant>,
    mouse_note: &mut Option<i32>,
) {
    synthesizer.note_off_all(false);
    active_key_notes.clear();
    active_notes.clear();
    legacy_note_off_deadlines.clear();
    sustained_note_off_deadlines.clear();
    key_last_seen.clear();
    *mouse_note = None;
}

fn release_or_sustain_note(
    note: i32,
    sustain_enabled: bool,
    synthesizer: &mut rustysynth::Synthesizer,
    active_key_notes: &HashMap<char, i32>,
    active_notes: &mut HashSet<i32>,
    legacy_note_off_deadlines: &HashMap<i32, Instant>,
    sustained_note_off_deadlines: &mut HashMap<i32, Instant>,
    mouse_note: Option<i32>,
) {
    let held_by_keyboard = active_key_notes.values().any(|n| *n == note);
    let held_by_legacy = legacy_note_off_deadlines
        .get(&note)
        .is_some_and(|deadline| Instant::now() < *deadline);
    let held_by_mouse = mouse_note == Some(note);
    if held_by_keyboard || held_by_legacy || held_by_mouse {
        return;
    }

    if sustain_enabled {
        sustained_note_off_deadlines.insert(note, Instant::now() + SUSTAIN_TAIL_DURATION);
        active_notes.insert(note);
    } else if active_notes.remove(&note) {
        synthesizer.note_off(SYNTH_CHANNEL, note);
    }
}

fn sync_picker_state(
    picker_selected: &mut usize,
    picker_scroll: &mut usize,
    filtered_len: usize,
    visible_rows: usize,
) {
    if filtered_len == 0 {
        *picker_selected = 0;
        *picker_scroll = 0;
        return;
    }

    *picker_selected = (*picker_selected).min(filtered_len - 1);
    if *picker_selected < *picker_scroll {
        *picker_scroll = *picker_selected;
    }
    if *picker_selected >= *picker_scroll + visible_rows {
        *picker_scroll = (*picker_selected)
            .saturating_add(1)
            .saturating_sub(visible_rows);
    }
}

fn truncate_and_pad(input: &str, width: usize) -> String {
    let mut out: String = input.chars().take(width).collect();
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

fn picker_visible_rows_for_height(total_height: usize) -> usize {
    let reserved = PIANO_HEIGHT + 1 + 1 + HELP_HEIGHT + 4;
    total_height
        .saturating_sub(reserved)
        .max(1)
        .min(PICKER_MAX_VISIBLE)
}

fn help_row(
    pad: &str,
    muted: Style,
    very_muted: Style,
    left_key: &str,
    left_desc: &str,
    right_key: &str,
    right_desc: &str,
    left_col_width: usize,
) -> Line<'static> {
    let left = if left_key.is_empty() {
        left_desc.to_string()
    } else if left_desc.is_empty() {
        left_key.to_string()
    } else {
        format!("{left_key} {left_desc}")
    };
    let left_padded = truncate_and_pad(&left, left_col_width);
    let left_key_len = left_key.chars().count();
    let left_desc_len = left_desc.chars().count();
    let left_total_len = if left_key.is_empty() || left_desc.is_empty() {
        left_key_len + left_desc_len
    } else {
        left_key_len + 1 + left_desc_len
    };
    let left_gap_len = left_padded.chars().count().saturating_sub(left_total_len);
    let left_gap = " ".repeat(left_gap_len);
    let left_desc_with_prefix = if left_key.is_empty() || left_desc.is_empty() {
        left_desc.to_string()
    } else {
        format!(" {left_desc}")
    };

    Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(left_key.to_string(), muted),
        Span::styled(format!("{left_desc_with_prefix}{left_gap}"), very_muted),
        Span::styled(right_key.to_string(), muted),
        Span::styled(
            if right_key.is_empty() || right_desc.is_empty() {
                right_desc.to_string()
            } else {
                format!(" {right_desc}")
            },
            very_muted,
        ),
    ])
}

fn note_at_piano_cell(
    column: usize,
    row: usize,
    left_pad: usize,
    visible_white_notes: &[i32],
) -> Option<i32> {
    if row >= PIANO_HEIGHT {
        return None;
    }

    let width = piano_width(visible_white_notes.len());
    if column < left_pad || column >= left_pad + width {
        return None;
    }
    let x = column - left_pad;

    if (1..=6).contains(&row) {
        for (i, white) in visible_white_notes.iter().enumerate() {
            let Some(black) = black_after_white(*white).filter(|n| *n <= MIDI_HIGH_NOTE) else {
                continue;
            };
            let center = (i + 1) * PIANO_KEY_WIDTH;
            let left = center.saturating_sub(1);
            let right = (center + 1).min(width - 1);
            if (left..=right).contains(&x) {
                return Some(black);
            }
        }
    }

    if (1..=(PIANO_HEIGHT - 2)).contains(&row) {
        let idx = x / PIANO_KEY_WIDTH;
        return visible_white_notes.get(idx).copied();
    }

    None
}

fn build_piano_lines(
    active_notes: &HashSet<i32>,
    visible_white_notes: &[i32],
    keyboard_base_note: i32,
    left_pad: usize,
) -> Vec<Line<'static>> {
    let white_count = visible_white_notes.len();
    let key_w = PIANO_KEY_WIDTH;
    let width = white_count * key_w + 1;
    let height = PIANO_HEIGHT;

    let mut chars = vec![vec![' '; width + left_pad]; height];
    let mut styles = vec![vec![Style::default(); width + left_pad]; height];

    let border_style = Style::default().fg(Color::Gray);

    for i in 0..white_count {
        let note = visible_white_notes[i];
        let pressed = active_notes.contains(&note);
        let fill_style = if pressed {
            Style::default().fg(Color::White).bg(Color::Red)
        } else {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(230, 230, 230))
        };

        let x0 = left_pad + i * key_w;
        let x1 = x0 + key_w;

        chars[0][x0] = if i == 0 { '┌' } else { '┬' };
        styles[0][x0] = border_style;
        for x in (x0 + 1)..x1 {
            chars[0][x] = '─';
            styles[0][x] = border_style;
        }

        for y in 1..(height - 1) {
            chars[y][x0] = '│';
            styles[y][x0] = border_style;
            for x in (x0 + 1)..x1 {
                chars[y][x] = ' ';
                styles[y][x] = fill_style;
            }
        }

        if i == white_count - 1 {
            chars[0][left_pad + width - 1] = '┐';
            styles[0][left_pad + width - 1] = border_style;
            for y in 1..(height - 1) {
                chars[y][left_pad + width - 1] = '│';
                styles[y][left_pad + width - 1] = border_style;
            }
        }

        let label = display_label_for_note(note, keyboard_base_note);
        if label != ' ' {
            chars[height - 3][x0 + 2] = label;
            styles[height - 3][x0 + 2] = Style::default()
                .fg(Color::Black)
                .bg(fill_style.bg.unwrap_or(Color::Reset));
        }

        if note.rem_euclid(12) == 0 {
            let octave = note.div_euclid(12) - 1;
            let label = format!("C{octave}");
            for (offset, c) in label.chars().take(key_w - 1).enumerate() {
                chars[height - 2][x0 + 1 + offset] = c;
                styles[height - 2][x0 + 1 + offset] = Style::default()
                    .fg(Color::Black)
                    .bg(fill_style.bg.unwrap_or(Color::Reset));
            }
        }
    }

    chars[height - 1][left_pad] = '└';
    styles[height - 1][left_pad] = border_style;
    for i in 0..white_count {
        let x0 = left_pad + i * key_w;
        for x in (x0 + 1)..(x0 + key_w) {
            chars[height - 1][x] = '─';
            styles[height - 1][x] = border_style;
        }
        chars[height - 1][x0 + key_w] = if i == white_count - 1 { '┘' } else { '┴' };
        styles[height - 1][x0 + key_w] = border_style;
    }

    for i in 0..white_count {
        let white = visible_white_notes[i];
        let Some(black) = black_after_white(white).filter(|n| *n <= MIDI_HIGH_NOTE) else {
            continue;
        };
        let pressed = active_notes.contains(&black);
        let black_fill = if pressed {
            Style::default().fg(Color::Blue)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let center = left_pad + (i + 1) * key_w;
        let left = center.saturating_sub(1);
        let right = (center + 1).min(left_pad + width - 1);

        chars[0][left] = '┬';
        chars[0][center] = '─';
        chars[0][right] = '┬';
        styles[0][left] = border_style;
        styles[0][center] = border_style;
        styles[0][right] = border_style;

        for y in 1..7 {
            chars[y][left] = if y == 6 { '└' } else { '│' };
            chars[y][center] = if y == 6 { '─' } else { '█' };
            chars[y][right] = if y == 6 { '┘' } else { '│' };
            styles[y][left] = border_style;
            styles[y][center] = if y == 6 { border_style } else { black_fill };
            styles[y][right] = border_style;
        }
    }

    let mut out = Vec::new();
    for y in 0..height {
        out.push(row_to_line(&chars[y], &styles[y]));
    }
    out
}

fn pump_audio(
    audio_writer: &mut AudioWriter,
    synthesizer: &mut rustysynth::Synthesizer,
    left: &mut Vec<f32>,
    right: &mut Vec<f32>,
    interleaved: &mut Vec<f32>,
) {
    let needed_frames = audio_writer.should_write();
    if needed_frames == 0 {
        return;
    }

    left.resize(needed_frames, 0.0);
    right.resize(needed_frames, 0.0);
    synthesizer.render(left, right);

    interleaved.clear();
    interleaved.reserve(needed_frames * 2);
    for (&l, &r) in left.iter().zip(right.iter()) {
        interleaved.push(l);
        interleaved.push(r);
    }

    let written = audio::write(interleaved);
    if written > 0 {
        audio_writer.next_pts += written as u64;
    }
}

fn main() -> std::io::Result<()> {
    let mut terminal = Terminal::new(TerminalGamesBackend::new(std::io::stdout()))?;
    terminal.clear()?;
    let mut enhanced_input = terminal_likely_supports_kitty_protocol();
    if enhanced_input {
        std::io::stdout().write(KITTY_KEYBOARD_ENABLE)?;
        std::io::stdout().flush()?;
    }
    std::io::stdout().write(MOUSE_ENABLE)?;
    std::io::stdout().flush()?;

    let mut sf = SOUNDFONT;
    let sound_font = Arc::new(rustysynth::SoundFont::new(&mut sf).unwrap());
    let presets: Vec<PresetChoice> = sound_font
        .get_presets()
        .iter()
        .map(|preset| PresetChoice {
            name: preset.get_name().to_string(),
            bank: preset.get_bank_number(),
            patch: preset.get_patch_number(),
        })
        .collect();
    let mut current_preset_index = presets
        .iter()
        .position(|preset| preset.bank == 0 && preset.patch == 0)
        .unwrap_or(0);

    let settings =
        rustysynth::SynthesizerSettings::new(terminal_games_sdk::audio::SAMPLE_RATE as i32);
    let mut synthesizer = rustysynth::Synthesizer::new(&sound_font, &settings).unwrap();
    if let Some(preset) = presets.get(current_preset_index) {
        apply_preset(&mut synthesizer, preset);
    }
    let mut audio_writer = AudioWriter::default();
    let mut left = Vec::<f32>::new();
    let mut right = Vec::<f32>::new();
    let mut interleaved = Vec::<f32>::new();
    let mut active_key_notes = HashMap::<char, i32>::new();
    let mut active_notes = HashSet::<i32>::new();
    let mut legacy_note_off_deadlines = HashMap::<i32, Instant>::new();
    let mut sustained_note_off_deadlines = HashMap::<i32, Instant>::new();
    let mut key_last_seen = HashMap::<char, Instant>::new();
    let mut mouse_note = None::<i32>;
    let mut sustain_enabled = false;
    let (min_octave_shift, max_octave_shift) = octave_shift_bounds();
    let mut octave_shift = 0;
    let all_white_notes = white_notes();
    let mut piano_scroll = all_white_notes
        .iter()
        .position(|note| *note == KEYBOARD_BASE_NOTE)
        .unwrap_or(0);
    let mut picker_open = false;
    let mut preset_filter = String::new();
    let mut picker_selected = 0usize;
    let mut picker_scroll = 0usize;
    let mut prev_content_width = 0usize;

    let mut terminal_reader = TerminalReader {};
    let mut next_frame = std::time::Instant::now();

    'outer: loop {
        if app::graceful_shutdown_poll() {
            break;
        }
        let area = terminal.size()?;
        let (content_x, content_y, content_width, content_height) =
            content_viewport(area.width, area.height);
        let white_count = visible_white_count(content_width, all_white_notes.len());
        let max_piano_scroll = all_white_notes.len().saturating_sub(white_count);
        let keyboard_base_note = KEYBOARD_BASE_NOTE + octave_shift * 12;
        let resized = content_width != prev_content_width;
        let focus_note = if resized {
            if active_notes.is_empty() {
                Some(keyboard_base_note + KEYBOARD_MAX_SEMITONE / 2)
            } else {
                Some(active_notes.iter().copied().sum::<i32>() / active_notes.len() as i32)
            }
        } else {
            None
        };
        piano_scroll = piano_scroll_with_playable_keys_visible(
            &all_white_notes,
            piano_scroll,
            white_count,
            keyboard_base_note,
            focus_note,
        );
        let visible_white_notes = &all_white_notes[piano_scroll..(piano_scroll + white_count)];
        let piano_left = piano_left_offset(content_width, white_count, all_white_notes.len());
        let (ui_left, ui_width) = centered_column(content_width, MAX_UI_COLUMN_WIDTH);
        prev_content_width = content_width;

        for event in &mut terminal_reader {
            if let Some(key_event) = event.as_key() {
                let key_active = key_event.kind != terminput::KeyEventKind::Release;
                let picker_visible_rows = picker_visible_rows_for_height(content_height);

                if key_active && matches!(key_event.code, terminput::KeyCode::Esc) {
                    if picker_open {
                        picker_open = false;
                    } else {
                        break 'outer;
                    }
                    continue;
                }

                if key_active && !picker_open && matches!(key_event.code, terminput::KeyCode::Enter)
                {
                    picker_open = true;
                    let filtered = filter_preset_indices(&presets, &preset_filter);
                    picker_selected = filtered
                        .iter()
                        .position(|idx| *idx == current_preset_index)
                        .unwrap_or(0);
                    sync_picker_state(
                        &mut picker_selected,
                        &mut picker_scroll,
                        filtered.len(),
                        picker_visible_rows,
                    );
                    continue;
                }

                if picker_open {
                    let mut filtered = filter_preset_indices(&presets, &preset_filter);
                    sync_picker_state(
                        &mut picker_selected,
                        &mut picker_scroll,
                        filtered.len(),
                        picker_visible_rows,
                    );

                    match key_event.code {
                        terminput::KeyCode::Up if key_active => {
                            picker_selected = picker_selected.saturating_sub(1);
                        }
                        terminput::KeyCode::Down if key_active => {
                            if picker_selected + 1 < filtered.len() {
                                picker_selected += 1;
                            }
                        }
                        terminput::KeyCode::Enter if key_active => {
                            if let Some(&idx) = filtered.get(picker_selected) {
                                if idx != current_preset_index {
                                    current_preset_index = idx;
                                    clear_playing_notes(
                                        &mut synthesizer,
                                        &mut active_key_notes,
                                        &mut active_notes,
                                        &mut legacy_note_off_deadlines,
                                        &mut sustained_note_off_deadlines,
                                        &mut key_last_seen,
                                        &mut mouse_note,
                                    );
                                    apply_preset(&mut synthesizer, &presets[current_preset_index]);
                                }
                            }
                            picker_open = false;
                        }
                        terminput::KeyCode::Backspace if key_active => {
                            preset_filter.pop();
                            filtered = filter_preset_indices(&presets, &preset_filter);
                            picker_selected = 0;
                            picker_scroll = 0;
                        }
                        terminput::KeyCode::Char(c)
                            if matches!(
                                key_event.kind,
                                terminput::KeyEventKind::Press | terminput::KeyEventKind::Repeat
                            ) && !c.is_control() =>
                        {
                            preset_filter.push(c);
                            filtered = filter_preset_indices(&presets, &preset_filter);
                            picker_selected = 0;
                            picker_scroll = 0;
                        }
                        _ => {}
                    }

                    sync_picker_state(
                        &mut picker_selected,
                        &mut picker_scroll,
                        filtered.len(),
                        picker_visible_rows,
                    );
                    continue;
                }

                match key_event.code {
                    terminput::KeyCode::Char(' ') if key_active => {
                        sustain_enabled = !sustain_enabled;
                        if !sustain_enabled {
                            let sustained_notes: Vec<i32> =
                                sustained_note_off_deadlines.keys().copied().collect();
                            for note in sustained_notes {
                                release_or_sustain_note(
                                    note,
                                    false,
                                    &mut synthesizer,
                                    &active_key_notes,
                                    &mut active_notes,
                                    &legacy_note_off_deadlines,
                                    &mut sustained_note_off_deadlines,
                                    mouse_note,
                                );
                            }
                            sustained_note_off_deadlines.clear();
                        }
                    }
                    terminput::KeyCode::Char('`') if key_active => {
                        enhanced_input = !enhanced_input;
                        if enhanced_input {
                            std::io::stdout().write(KITTY_KEYBOARD_ENABLE)?;
                        } else {
                            std::io::stdout().write(KITTY_KEYBOARD_DISABLE)?;
                        }
                        std::io::stdout().flush()?;

                        clear_playing_notes(
                            &mut synthesizer,
                            &mut active_key_notes,
                            &mut active_notes,
                            &mut legacy_note_off_deadlines,
                            &mut sustained_note_off_deadlines,
                            &mut key_last_seen,
                            &mut mouse_note,
                        );
                    }
                    terminput::KeyCode::Char('a') if key_active => {
                        if octave_shift > min_octave_shift {
                            octave_shift -= 1;
                        }
                    }
                    terminput::KeyCode::Char(';') if key_active => {
                        if octave_shift < max_octave_shift {
                            octave_shift += 1;
                        }
                    }
                    terminput::KeyCode::Left if key_active => {
                        piano_scroll = piano_scroll.saturating_sub(1);
                    }
                    terminput::KeyCode::Right if key_active => {
                        if piano_scroll < max_piano_scroll {
                            piano_scroll += 1;
                        }
                    }
                    terminput::KeyCode::Char(c) => {
                        let c = c.to_ascii_lowercase();
                        if enhanced_input {
                            match key_event.kind {
                                terminput::KeyEventKind::Press => {
                                    let Some(note) =
                                        key_to_note(c, KEYBOARD_BASE_NOTE + octave_shift * 12)
                                    else {
                                        continue;
                                    };
                                    if let std::collections::hash_map::Entry::Vacant(e) =
                                        active_key_notes.entry(c)
                                    {
                                        e.insert(note);
                                        sustained_note_off_deadlines.remove(&note);
                                        active_notes.insert(note);
                                        synthesizer.note_on(SYNTH_CHANNEL, note, 110);
                                    }
                                    key_last_seen.insert(c, Instant::now());
                                }
                                terminput::KeyEventKind::Release => {
                                    let Some(note) = active_key_notes.remove(&c) else {
                                        continue;
                                    };
                                    key_last_seen.remove(&c);
                                    release_or_sustain_note(
                                        note,
                                        sustain_enabled,
                                        &mut synthesizer,
                                        &active_key_notes,
                                        &mut active_notes,
                                        &legacy_note_off_deadlines,
                                        &mut sustained_note_off_deadlines,
                                        mouse_note,
                                    );
                                }
                                terminput::KeyEventKind::Repeat => {
                                    key_last_seen.insert(c, Instant::now());
                                }
                            }
                        } else {
                            match key_event.kind {
                                terminput::KeyEventKind::Press
                                | terminput::KeyEventKind::Repeat => {
                                    let Some(note) =
                                        key_to_note(c, KEYBOARD_BASE_NOTE + octave_shift * 12)
                                    else {
                                        continue;
                                    };
                                    sustained_note_off_deadlines.remove(&note);
                                    active_notes.insert(note);
                                    synthesizer.note_on(SYNTH_CHANNEL, note, 110);
                                    legacy_note_off_deadlines
                                        .insert(note, Instant::now() + LEGACY_NOTE_DURATION);
                                }
                                terminput::KeyEventKind::Release => {
                                    let Some(note) =
                                        key_to_note(c, KEYBOARD_BASE_NOTE + octave_shift * 12)
                                    else {
                                        continue;
                                    };
                                    legacy_note_off_deadlines.remove(&note);
                                    release_or_sustain_note(
                                        note,
                                        sustain_enabled,
                                        &mut synthesizer,
                                        &active_key_notes,
                                        &mut active_notes,
                                        &legacy_note_off_deadlines,
                                        &mut sustained_note_off_deadlines,
                                        mouse_note,
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let Some(mouse) = event.as_mouse() {
                let row = mouse.row as usize;
                let col = mouse.column as usize;
                if row < content_y
                    || row >= content_y + content_height
                    || col < content_x
                    || col >= content_x + content_width
                {
                    continue;
                }

                let content_row = row - content_y;
                let content_col = col - content_x;
                let picker_visible_rows = picker_visible_rows_for_height(content_height);
                let picker_x = ui_left;
                let instrument_line_row = PIANO_HEIGHT + 1;
                let picker_y = instrument_line_row + 1;
                let picker_width = ui_width.saturating_sub(2).min(58);
                let list_top = picker_y + 3;
                let list_bottom = list_top + picker_visible_rows;

                if matches!(
                    mouse.kind,
                    terminput::MouseEventKind::Down(terminput::MouseButton::Left)
                ) && content_row == instrument_line_row
                    && content_col >= ui_left
                    && content_col < ui_left + ui_width
                {
                    picker_open = !picker_open;
                    if picker_open {
                        let filtered = filter_preset_indices(&presets, &preset_filter);
                        picker_selected = filtered
                            .iter()
                            .position(|idx| *idx == current_preset_index)
                            .unwrap_or(0);
                        sync_picker_state(
                            &mut picker_selected,
                            &mut picker_scroll,
                            filtered.len(),
                            picker_visible_rows,
                        );
                    }
                    continue;
                }

                if picker_open {
                    if matches!(
                        mouse.kind,
                        terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up)
                            | terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down)
                    ) {
                        let filtered = filter_preset_indices(&presets, &preset_filter);
                        if !filtered.is_empty() {
                            match mouse.kind {
                                terminput::MouseEventKind::Scroll(
                                    terminput::ScrollDirection::Up,
                                ) => {
                                    picker_selected = picker_selected.saturating_sub(1);
                                }
                                terminput::MouseEventKind::Scroll(
                                    terminput::ScrollDirection::Down,
                                ) => {
                                    if picker_selected + 1 < filtered.len() {
                                        picker_selected += 1;
                                    }
                                }
                                _ => {}
                            }
                            sync_picker_state(
                                &mut picker_selected,
                                &mut picker_scroll,
                                filtered.len(),
                                picker_visible_rows,
                            );
                        }
                    }

                    if matches!(
                        mouse.kind,
                        terminput::MouseEventKind::Down(terminput::MouseButton::Left)
                    ) && picker_width >= 24
                        && content_row >= list_top
                        && content_row < list_bottom
                        && content_col > picker_x
                        && content_col < picker_x + picker_width - 1
                    {
                        let filtered = filter_preset_indices(&presets, &preset_filter);
                        let click_idx = picker_scroll + (content_row - list_top);
                        if let Some(&idx) = filtered.get(click_idx) {
                            picker_selected = click_idx;
                            if idx != current_preset_index {
                                current_preset_index = idx;
                                clear_playing_notes(
                                    &mut synthesizer,
                                    &mut active_key_notes,
                                    &mut active_notes,
                                    &mut legacy_note_off_deadlines,
                                    &mut sustained_note_off_deadlines,
                                    &mut key_last_seen,
                                    &mut mouse_note,
                                );
                                apply_preset(&mut synthesizer, &presets[current_preset_index]);
                            }
                            picker_open = false;
                        }
                    }
                    continue;
                }

                if matches!(
                    mouse.kind,
                    terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up)
                        | terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down)
                ) && content_row < PIANO_HEIGHT
                    && content_col >= piano_left
                    && content_col < piano_left + piano_width(visible_white_notes.len())
                {
                    match mouse.kind {
                        terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up) => {
                            piano_scroll = piano_scroll.saturating_sub(1);
                        }
                        terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down) => {
                            if piano_scroll < max_piano_scroll {
                                piano_scroll += 1;
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                let hit =
                    note_at_piano_cell(content_col, content_row, piano_left, visible_white_notes);
                match mouse.kind {
                    terminput::MouseEventKind::Down(terminput::MouseButton::Left)
                    | terminput::MouseEventKind::Drag(terminput::MouseButton::Left) => {
                        if mouse_note != hit {
                            if let Some(old) = mouse_note.take() {
                                release_or_sustain_note(
                                    old,
                                    sustain_enabled,
                                    &mut synthesizer,
                                    &active_key_notes,
                                    &mut active_notes,
                                    &legacy_note_off_deadlines,
                                    &mut sustained_note_off_deadlines,
                                    mouse_note,
                                );
                            }

                            if let Some(new_note) = hit {
                                mouse_note = Some(new_note);
                                sustained_note_off_deadlines.remove(&new_note);
                                active_notes.insert(new_note);
                                synthesizer.note_on(SYNTH_CHANNEL, new_note, 110);
                            }
                        }
                    }
                    terminput::MouseEventKind::Up(terminput::MouseButton::Left) => {
                        if let Some(old) = mouse_note.take() {
                            release_or_sustain_note(
                                old,
                                sustain_enabled,
                                &mut synthesizer,
                                &active_key_notes,
                                &mut active_notes,
                                &legacy_note_off_deadlines,
                                &mut sustained_note_off_deadlines,
                                mouse_note,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        {
            let picker_visible_rows = picker_visible_rows_for_height(content_height);
            let filtered = filter_preset_indices(&presets, &preset_filter);
            sync_picker_state(
                &mut picker_selected,
                &mut picker_scroll,
                filtered.len(),
                picker_visible_rows,
            );
        }
        piano_scroll = piano_scroll_with_playable_keys_visible(
            &all_white_notes,
            piano_scroll,
            white_count,
            KEYBOARD_BASE_NOTE + octave_shift * 12,
            None,
        );

        if enhanced_input {
            let now = Instant::now();
            let mut stale = Vec::new();
            for (&k, &last_seen) in &key_last_seen {
                if now.duration_since(last_seen) >= ENHANCED_STUCK_TIMEOUT {
                    stale.push(k);
                }
            }
            for k in stale {
                if let Some(note) = active_key_notes.remove(&k) {
                    key_last_seen.remove(&k);
                    release_or_sustain_note(
                        note,
                        sustain_enabled,
                        &mut synthesizer,
                        &active_key_notes,
                        &mut active_notes,
                        &legacy_note_off_deadlines,
                        &mut sustained_note_off_deadlines,
                        mouse_note,
                    );
                }
            }
        } else {
            let now = Instant::now();
            let mut to_stop = Vec::new();
            for (&note, &deadline) in &legacy_note_off_deadlines {
                if now >= deadline {
                    to_stop.push(note);
                }
            }
            for note in to_stop {
                legacy_note_off_deadlines.remove(&note);
                release_or_sustain_note(
                    note,
                    sustain_enabled,
                    &mut synthesizer,
                    &active_key_notes,
                    &mut active_notes,
                    &legacy_note_off_deadlines,
                    &mut sustained_note_off_deadlines,
                    mouse_note,
                );
            }
        }

        {
            let now = Instant::now();
            let mut to_stop = Vec::new();
            for (&note, &deadline) in &sustained_note_off_deadlines {
                if now >= deadline {
                    to_stop.push(note);
                }
            }
            for note in to_stop {
                sustained_note_off_deadlines.remove(&note);
                release_or_sustain_note(
                    note,
                    false,
                    &mut synthesizer,
                    &active_key_notes,
                    &mut active_notes,
                    &legacy_note_off_deadlines,
                    &mut sustained_note_off_deadlines,
                    mouse_note,
                );
            }
        }

        pump_audio(
            &mut audio_writer,
            &mut synthesizer,
            &mut left,
            &mut right,
            &mut interleaved,
        );

        terminal.draw(|frame| {
            let area = frame.area();
            let content = content_area(area);
            let white_count = visible_white_count(content.width as usize, all_white_notes.len());
            let piano_scroll = piano_scroll_with_playable_keys_visible(
                &all_white_notes,
                piano_scroll,
                white_count,
                KEYBOARD_BASE_NOTE + octave_shift * 12,
                None,
            );
            let visible_white_notes = &all_white_notes[piano_scroll..(piano_scroll + white_count)];
            let keyboard_base_note = KEYBOARD_BASE_NOTE + octave_shift * 12;
            let piano_left =
                piano_left_offset(content.width as usize, white_count, all_white_notes.len());
            let (ui_left, ui_width) = centered_column(content.width as usize, MAX_UI_COLUMN_WIDTH);
            let mut lines = Vec::new();
            lines.extend(build_piano_lines(
                &active_notes,
                visible_white_notes,
                keyboard_base_note,
                piano_left,
            ));
            let instrument = presets
                .get(current_preset_index)
                .map(|preset| {
                    format!(
                        "{} (bank {}, patch {})",
                        preset.name, preset.bank, preset.patch
                    )
                })
                .unwrap_or_else(|| "N/A".to_string());
            let pad = " ".repeat(ui_left);
            let picker_width = ui_width.saturating_sub(2).min(58);
            let picker_visible_rows = picker_visible_rows_for_height(content.height as usize);
            let muted = Style::default().fg(Color::Gray);
            let very_muted = Style::default().fg(Color::DarkGray);

            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled(
                    format!(
                        "{} Instrument: {}",
                        if picker_open { "▾" } else { "▸" },
                        instrument
                    ),
                    Style::default().fg(Color::Gray),
                ),
            ]));

            if picker_open && picker_width >= 24 {
                let filtered = filter_preset_indices(&presets, &preset_filter);
                let inner = picker_width - 2;
                let border_style = Style::default().fg(Color::Gray);
                lines.push(Line::from(vec![
                    Span::raw(pad.to_string()),
                    Span::styled(format!("┌{}┐", "─".repeat(inner)), border_style),
                ]));
                lines.push(Line::from(vec![
                    Span::raw(pad.to_string()),
                    Span::styled("│", border_style),
                    Span::styled(
                        truncate_and_pad(&format!(" Filter: {}", preset_filter), inner),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::styled("│", border_style),
                ]));
                lines.push(Line::from(vec![
                    Span::raw(pad.to_string()),
                    Span::styled(format!("├{}┤", "─".repeat(inner)), border_style),
                ]));

                if filtered.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(pad.to_string()),
                        Span::styled("│", border_style),
                        Span::styled(
                            truncate_and_pad(" No matches", inner),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled("│", border_style),
                    ]));
                    for _ in 1..picker_visible_rows {
                        lines.push(Line::from(vec![
                            Span::raw(pad.to_string()),
                            Span::styled("│", border_style),
                            Span::styled(" ".repeat(inner), Style::default().fg(Color::Gray)),
                            Span::styled("│", border_style),
                        ]));
                    }
                } else {
                    for row in 0..picker_visible_rows {
                        let idx = picker_scroll + row;
                        if let Some(preset_idx) = filtered.get(idx) {
                            let preset = &presets[*preset_idx];
                            let marker = if idx == picker_selected { "▶" } else { " " };
                            let label = format!(
                                " {} {}  [{:03}:{:03}]",
                                marker, preset.name, preset.bank, preset.patch
                            );
                            let style = if idx == picker_selected {
                                Style::default().fg(Color::White)
                            } else {
                                Style::default().fg(Color::Gray)
                            };
                            lines.push(Line::from(vec![
                                Span::raw(pad.to_string()),
                                Span::styled("│", border_style),
                                Span::styled(truncate_and_pad(&label, inner), style),
                                Span::styled("│", border_style),
                            ]));
                        } else {
                            lines.push(Line::from(vec![
                                Span::raw(pad.to_string()),
                                Span::styled("│", border_style),
                                Span::styled(" ".repeat(inner), Style::default().fg(Color::Gray)),
                                Span::styled("│", border_style),
                            ]));
                        }
                    }
                }
                lines.push(Line::from(vec![
                    Span::raw(pad.to_string()),
                    Span::styled(format!("└{}┘", "─".repeat(inner)), border_style),
                ]));
            }

            let sustain_state = if sustain_enabled { "on" } else { "off" };
            let protocol_mode = if enhanced_input { "kitty" } else { "normal" };
            let left_col_width = 29;
            let help_lines = if picker_open {
                vec![
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "up/down",
                        "move selection",
                        "enter",
                        "choose instrument",
                        left_col_width,
                    ),
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "type",
                        "filter text",
                        "backspace",
                        "delete char",
                        left_col_width,
                    ),
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "esc",
                        "close picker",
                        "space",
                        &format!("sustain {sustain_state}"),
                        left_col_width,
                    ),
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "`",
                        &format!("input protocol: {protocol_mode}"),
                        "",
                        "",
                        left_col_width,
                    ),
                ]
            } else {
                vec![
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "a/;",
                        "shift keyboard octave",
                        "left/right",
                        "pan piano roll",
                        left_col_width,
                    ),
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "`",
                        &format!("input protocol: {protocol_mode}"),
                        "",
                        "",
                        left_col_width,
                    ),
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "space",
                        &format!("sustain {sustain_state}"),
                        "enter",
                        "instrument picker",
                        left_col_width,
                    ),
                    help_row(
                        &pad,
                        muted,
                        very_muted,
                        "esc",
                        "quit",
                        "",
                        "",
                        left_col_width,
                    ),
                ]
            };
            let reserved_bottom = help_lines.len();
            if (content.height as usize) > reserved_bottom
                && lines.len() < (content.height as usize) - reserved_bottom
            {
                let blanks = (content.height as usize) - reserved_bottom - lines.len();
                for _ in 0..blanks {
                    lines.push(Line::from(""));
                }
            }
            lines.extend(help_lines);
            frame.render_widget(Paragraph::new(lines), content);
        })?;

        next_frame += FRAME_DURATION;
        loop {
            pump_audio(
                &mut audio_writer,
                &mut synthesizer,
                &mut left,
                &mut right,
                &mut interleaved,
            );
            mixer().tick();
            let now = std::time::Instant::now();
            let Some(remaining) = next_frame.checked_duration_since(now) else {
                break;
            };

            if remaining <= MIXER_TICK_INTERVAL {
                std::thread::sleep(remaining);
                break;
            }

            std::thread::sleep(MIXER_TICK_INTERVAL);
        }
    }

    std::io::stdout().write(MOUSE_DISABLE)?;
    std::io::stdout().flush()?;
    if enhanced_input {
        std::io::stdout().write(KITTY_KEYBOARD_DISABLE)?;
        std::io::stdout().flush()?;
    }
    Ok(())
}
