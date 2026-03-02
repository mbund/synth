use crate::{
    KEYBOARD_BASE_NOTE, KEYBOARD_MAX_SEMITONE, MIDI_HIGH_NOTE, MIDI_LOW_NOTE,
    MIN_CONTENT_HEIGHT_FOR_PADDING, MIN_CONTENT_WIDTH_FOR_PADDING, PIANO_HEIGHT, PIANO_KEY_WIDTH,
};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};
use std::collections::{HashMap, HashSet};

pub fn key_to_semitone(c: char) -> Option<i32> {
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

pub fn octave_shift_bounds() -> (i32, i32) {
    let min = (MIDI_LOW_NOTE - KEYBOARD_BASE_NOTE).div_euclid(12);
    let max = (MIDI_HIGH_NOTE - KEYBOARD_BASE_NOTE - KEYBOARD_MAX_SEMITONE).div_euclid(12);
    (min, max)
}

pub fn key_to_note(c: char, keyboard_base_note: i32) -> Option<i32> {
    let semitone = key_to_semitone(c)?;
    let note = keyboard_base_note + semitone;
    (MIDI_LOW_NOTE..=MIDI_HIGH_NOTE).contains(&note).then_some(note)
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

pub fn is_white_key(note: i32) -> bool {
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

pub fn white_notes() -> Vec<i32> {
    (MIDI_LOW_NOTE..=MIDI_HIGH_NOTE)
        .filter(|n| is_white_key(*n))
        .collect()
}

pub fn visible_white_count(area_width: usize, total_white_keys: usize) -> usize {
    (area_width.saturating_sub(1) / PIANO_KEY_WIDTH)
        .max(1)
        .min(total_white_keys.max(1))
}

pub fn piano_width(visible_white_count: usize) -> usize {
    visible_white_count * PIANO_KEY_WIDTH + 1
}

pub fn piano_left_offset(
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

pub fn content_viewport(width: u16, height: u16) -> (usize, usize, usize, usize) {
    let pad = should_use_outer_padding(width as usize, height as usize) as usize;
    (
        pad,
        pad,
        (width as usize).saturating_sub(pad * 2),
        (height as usize).saturating_sub(pad * 2),
    )
}

pub fn content_area(area: Rect) -> Rect {
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

pub fn centered_column(container_width: usize, max_width: usize) -> (usize, usize) {
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

pub fn piano_scroll_with_playable_keys_visible(
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
        let focus_idx = white_index_for_note(all_white_notes, note.clamp(MIDI_LOW_NOTE, MIDI_HIGH_NOTE));
        focus_idx.saturating_sub(white_count / 2).min(max_scroll)
    } else {
        piano_scroll.min(max_scroll)
    };
    let low_note = keyboard_base_note.clamp(MIDI_LOW_NOTE, MIDI_HIGH_NOTE);
    let high_note = (keyboard_base_note + KEYBOARD_MAX_SEMITONE).clamp(MIDI_LOW_NOTE, MIDI_HIGH_NOTE);
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

pub fn note_at_piano_cell(
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

pub fn build_piano_lines(
    live_active_notes: &HashSet<i32>,
    loop_note_colors: &HashMap<i32, Color>,
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
        let fill_style = if live_active_notes.contains(&note) {
            Style::default().fg(Color::White).bg(Color::Red)
        } else if let Some(color) = loop_note_colors.get(&note) {
            Style::default().fg(Color::Black).bg(*color)
        } else {
            Style::default().fg(Color::Black).bg(Color::Rgb(230, 230, 230))
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
        let black_fill = if live_active_notes.contains(&black) {
            Style::default().fg(Color::Red)
        } else if let Some(color) = loop_note_colors.get(&black) {
            Style::default().fg(*color)
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
