use crate::{
    looping::loop_color,
    model::{LoopTrack, PresetChoice, UiFocus},
    piano::build_piano_lines,
    ui::{filter_preset_indices, help_row, picker_visible_rows_for_height, truncate_and_pad},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use std::collections::{HashMap, HashSet};
use terminal_games_sdk::terminput;

pub struct PianoWidget<'a> {
    pub live_active_notes: &'a HashSet<i32>,
    pub loop_note_colors: &'a HashMap<i32, Color>,
    pub visible_white_notes: &'a [i32],
    pub keyboard_base_note: i32,
    pub left_pad: usize,
}

impl Widget for PianoWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = build_piano_lines(
            self.live_active_notes,
            self.loop_note_colors,
            self.visible_white_notes,
            self.keyboard_base_note,
            self.left_pad,
        );
        Paragraph::new(lines).render(area, buf);
    }
}

pub struct InstrumentWidget<'a> {
    pub pad: &'a str,
    pub instrument: &'a str,
    pub focused: bool,
}

impl Widget for InstrumentWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let very_muted = Style::default().fg(Color::DarkGray);
        let lines = vec![Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled(
                format!("{} {}", if self.focused { "▸" } else { " " }, self.instrument),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("  enter", very_muted),
            Span::styled(" change", Style::default().fg(Color::Gray)),
        ])];
        Paragraph::new(lines).render(area, buf);
    }
}

pub struct PresetPickerWidget<'a> {
    pub pad: &'a str,
    pub presets: &'a [PresetChoice],
    pub preset_filter: &'a str,
    pub picker_selected: usize,
    pub picker_scroll: usize,
    pub picker_width: usize,
    pub picker_visible_rows: usize,
}

impl Widget for PresetPickerWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.picker_width < 24 {
            return;
        }
        let filtered = filter_preset_indices(self.presets, self.preset_filter);
        let inner = self.picker_width - 2;
        let border_style = Style::default().fg(Color::Gray);
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled(format!("┌{}┐", "─".repeat(inner)), border_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled("│", border_style),
            Span::styled(
                truncate_and_pad(&format!(" Filter: {}", self.preset_filter), inner),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("│", border_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled(format!("├{}┤", "─".repeat(inner)), border_style),
        ]));

        if filtered.is_empty() {
            lines.push(Line::from(vec![
                Span::raw(self.pad.to_string()),
                Span::styled("│", border_style),
                Span::styled(
                    truncate_and_pad(" No matches", inner),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled("│", border_style),
            ]));
            for _ in 1..self.picker_visible_rows {
                lines.push(Line::from(vec![
                    Span::raw(self.pad.to_string()),
                    Span::styled("│", border_style),
                    Span::styled(" ".repeat(inner), Style::default().fg(Color::Gray)),
                    Span::styled("│", border_style),
                ]));
            }
        } else {
            for row in 0..self.picker_visible_rows {
                let idx = self.picker_scroll + row;
                if let Some(preset_idx) = filtered.get(idx) {
                    let preset = &self.presets[*preset_idx];
                    let marker = if idx == self.picker_selected { "▶" } else { " " };
                    let label = format!(" {} {}  [{:03}:{:03}]", marker, preset.name, preset.bank, preset.patch);
                    let style = if idx == self.picker_selected {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    lines.push(Line::from(vec![
                        Span::raw(self.pad.to_string()),
                        Span::styled("│", border_style),
                        Span::styled(truncate_and_pad(&label, inner), style),
                        Span::styled("│", border_style),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(self.pad.to_string()),
                        Span::styled("│", border_style),
                        Span::styled(" ".repeat(inner), Style::default().fg(Color::Gray)),
                        Span::styled("│", border_style),
                    ]));
                }
            }
        }
        lines.push(Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled(format!("└{}┘", "─".repeat(inner)), border_style),
        ]));
        Paragraph::new(lines).render(area, buf);
    }
}

pub struct MetronomeWidget<'a> {
    pub pad: &'a str,
    pub focused: bool,
    pub bpm: u32,
    pub beats_per_bar: u8,
    pub enabled: bool,
}

impl Widget for MetronomeWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let very_muted = Style::default().fg(Color::DarkGray);
        let lines = vec![Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled(
                format!(
                    "{} {} BPM {}/bar",
                    if self.focused { "▸" } else { " " },
                    self.bpm,
                    self.beats_per_bar
                ),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("  enter", very_muted),
            Span::styled(
                format!(" {}", if self.enabled { "on" } else { "off" }),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("  -", very_muted),
            Span::styled(" bpm-", Style::default().fg(Color::Gray)),
            Span::styled("  +", very_muted),
            Span::styled(" bpm+", Style::default().fg(Color::Gray)),
            Span::styled("  ,", very_muted),
            Span::styled(" bar-", Style::default().fg(Color::Gray)),
            Span::styled("  .", very_muted),
            Span::styled(" bar+", Style::default().fg(Color::Gray)),
        ])];
        Paragraph::new(lines).render(area, buf);
    }
}

pub struct LoopListWidget<'a> {
    pub pad: &'a str,
    pub ui_focus: UiFocus,
    pub loop_tracks: &'a [LoopTrack],
}

impl Widget for LoopListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let very_muted = Style::default().fg(Color::DarkGray);
        let mut lines = Vec::new();
        for (idx, track) in self.loop_tracks.iter().enumerate() {
            let arrow = if self.ui_focus == UiFocus::Loop(idx) { "▸" } else { " " };
            let sustain_tag = if track.sustain_enabled { " sustain" } else { "" };
            lines.push(Line::from(vec![
                Span::raw(self.pad.to_string()),
                Span::styled(format!("{arrow} "), Style::default().fg(Color::Gray)),
                Span::styled("■", Style::default().fg(loop_color(idx))),
                Span::styled(
                    format!(" {}{}", track.instrument_name, sustain_tag),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(format!(" vol:{}%", track.volume_percent), Style::default().fg(Color::Gray)),
                Span::styled("  enter", very_muted),
                Span::styled(
                    format!(" {}", if track.enabled { "pause" } else { "play" }),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled("  backspace", very_muted),
                Span::styled(" delete", Style::default().fg(Color::Gray)),
                Span::styled(format!(" {} beats", track.beat_len), Style::default().fg(Color::Gray)),
            ]));
        }
        Paragraph::new(lines).render(area, buf);
    }
}

pub struct AddLoopWidget<'a> {
    pub pad: &'a str,
    pub focused: bool,
    pub recording: bool,
}

impl Widget for AddLoopWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let very_muted = Style::default().fg(Color::DarkGray);
        let lines = vec![Line::from(vec![
            Span::raw(self.pad.to_string()),
            Span::styled(
                if self.focused { "▸ " } else { "  " },
                Style::default().fg(Color::Gray),
            ),
            Span::styled("■", Style::default().fg(Color::DarkGray)),
            Span::styled(" Add Loop", Style::default().fg(Color::Gray)),
            Span::styled("  enter", very_muted),
            Span::styled(
                if self.recording { " save" } else { " record" },
                Style::default().fg(Color::Gray),
            ),
        ])];
        Paragraph::new(lines).render(area, buf);
    }
}

pub struct HelpWidget<'a> {
    pub pad: &'a str,
    pub sustain_enabled: bool,
    pub enhanced_input: bool,
}

impl Widget for HelpWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let muted = Style::default().fg(Color::Gray);
        let very_muted = Style::default().fg(Color::DarkGray);
        let protocol_mode = if self.enhanced_input { "kitty" } else { "normal" };
        let left_col_width = 34;
        let lines = vec![
            help_row(
                self.pad,
                muted,
                very_muted,
                "up/down",
                "move cursor",
                "left/right",
                "pan piano roll",
                left_col_width,
            ),
            help_row(
                self.pad,
                muted,
                very_muted,
                "a/;",
                "shift keyboard octave",
                "space",
                if self.sustain_enabled { "sustain on" } else { "sustain off" },
                left_col_width,
            ),
            help_row(
                self.pad,
                muted,
                very_muted,
                "`",
                &format!("input protocol: {protocol_mode}"),
                "esc",
                "quit",
                left_col_width,
            ),
        ];
        Paragraph::new(lines).render(area, buf);
    }
}

pub fn render_rows_for_layout(content_height: usize, picker_open: bool, picker_width: usize) -> usize {
    if picker_open && picker_width >= 24 {
        picker_visible_rows_for_height(content_height) + 4
    } else {
        0
    }
}

pub enum MetronomeMouseAction {
    None,
    ToggleEnabled,
    AdjustBpm(i8),
    AdjustBar(i8),
}

pub fn metronome_mouse_action(
    rel: usize,
    metronome_bpm: u32,
    metronome_beats_per_bar: u8,
    metronome_enabled: bool,
) -> MetronomeMouseAction {
    let prefix = format!("x {} BPM {}/bar", metronome_bpm, metronome_beats_per_bar);
    let mut cursor = prefix.chars().count();
    let toggle_segment = format!("  enter {}", if metronome_enabled { "on" } else { "off" });
    let toggle_end = cursor + toggle_segment.chars().count();
    if rel >= cursor && rel < toggle_end {
        return MetronomeMouseAction::ToggleEnabled;
    }
    cursor = toggle_end;

    let minus_segment = "  - bpm-";
    let minus_end = cursor + minus_segment.chars().count();
    if rel >= cursor && rel < minus_end {
        return MetronomeMouseAction::AdjustBpm(-1);
    }
    cursor = minus_end;

    let plus_segment = "  + bpm+";
    let plus_end = cursor + plus_segment.chars().count();
    if rel >= cursor && rel < plus_end {
        return MetronomeMouseAction::AdjustBpm(1);
    }
    cursor = plus_end;

    let bar_down_segment = "  , bar-";
    let bar_down_end = cursor + bar_down_segment.chars().count();
    if rel >= cursor && rel < bar_down_end {
        return MetronomeMouseAction::AdjustBar(-1);
    }
    cursor = bar_down_end;

    let bar_up_segment = "  . bar+";
    let bar_up_end = cursor + bar_up_segment.chars().count();
    if rel >= cursor && rel < bar_up_end {
        return MetronomeMouseAction::AdjustBar(1);
    }
    MetronomeMouseAction::None
}

pub enum LoopRowMouseAction {
    None,
    TogglePlayback,
    Delete,
}

pub fn loop_row_mouse_action(rel: usize, track: &LoopTrack) -> LoopRowMouseAction {
    let sustain_tag = if track.sustain_enabled { " sustain" } else { "" };
    let info = format!(" {}{}", track.instrument_name, sustain_tag);
    let hint = format!(
        " vol:{}%  enter {}  backspace delete  {} beats",
        track.volume_percent,
        if track.enabled { "pause" } else { "play" },
        track.beat_len
    );
    let hint_start = 2 + 1 + info.chars().count();
    let play_word = if track.enabled { "pause" } else { "play" };
    let play_start = hint_start + hint.find(play_word).unwrap_or(usize::MAX);
    let play_end = play_start + play_word.chars().count();
    let delete_start = hint_start + hint.find("delete").unwrap_or(usize::MAX);
    let delete_end = delete_start + "delete".chars().count();
    if rel >= play_start && rel < play_end {
        return LoopRowMouseAction::TogglePlayback;
    }
    if rel >= delete_start && rel < delete_end {
        return LoopRowMouseAction::Delete;
    }
    LoopRowMouseAction::None
}

pub enum InstrumentKeyResult {
    NotHandled,
    Handled,
    SelectPreset(usize),
}

pub fn handle_instrument_key(
    key_event: &terminput::KeyEvent,
    key_active: bool,
    ui_focus: UiFocus,
    picker_open: &mut bool,
    preset_filter: &mut String,
    picker_selected: &mut usize,
    picker_scroll: &mut usize,
    picker_visible_rows: usize,
    presets: &[PresetChoice],
    current_preset_index: usize,
) -> InstrumentKeyResult {
    if key_active
        && !*picker_open
        && ui_focus == UiFocus::Instrument
        && matches!(key_event.code, terminput::KeyCode::Enter)
    {
        *picker_open = true;
        let filtered = filter_preset_indices(presets, preset_filter);
        *picker_selected = filtered
            .iter()
            .position(|idx| *idx == current_preset_index)
            .unwrap_or(0);
        crate::ui::sync_picker_state(
            picker_selected,
            picker_scroll,
            filtered.len(),
            picker_visible_rows,
        );
        return InstrumentKeyResult::Handled;
    }

    if !*picker_open {
        return InstrumentKeyResult::NotHandled;
    }

    let mut filtered = filter_preset_indices(presets, preset_filter);
    crate::ui::sync_picker_state(
        picker_selected,
        picker_scroll,
        filtered.len(),
        picker_visible_rows,
    );

    let mut out = InstrumentKeyResult::Handled;
    match key_event.code {
        terminput::KeyCode::Up if key_active => {
            *picker_selected = picker_selected.saturating_sub(1);
        }
        terminput::KeyCode::Down if key_active => {
            if *picker_selected + 1 < filtered.len() {
                *picker_selected += 1;
            }
        }
        terminput::KeyCode::Enter if key_active => {
            if let Some(&idx) = filtered.get(*picker_selected) {
                out = InstrumentKeyResult::SelectPreset(idx);
            }
            *picker_open = false;
        }
        terminput::KeyCode::Backspace if key_active => {
            preset_filter.pop();
            filtered = filter_preset_indices(presets, preset_filter);
            *picker_selected = 0;
            *picker_scroll = 0;
        }
        terminput::KeyCode::Char(c)
            if matches!(
                key_event.kind,
                terminput::KeyEventKind::Press | terminput::KeyEventKind::Repeat
            ) && !c.is_control() =>
        {
            preset_filter.push(c);
            filtered = filter_preset_indices(presets, preset_filter);
            *picker_selected = 0;
            *picker_scroll = 0;
        }
        _ => {}
    }

    crate::ui::sync_picker_state(
        picker_selected,
        picker_scroll,
        filtered.len(),
        picker_visible_rows,
    );
    out
}

pub fn handle_metronome_key(
    key_event: &terminput::KeyEvent,
    key_active: bool,
    ui_focus: UiFocus,
    metronome_enabled: &mut bool,
    metronome_bpm: &mut u32,
    metronome_beats_per_bar: &mut u8,
    last_metronome_beat_emitted: &mut Option<u64>,
) -> bool {
    match key_event.code {
        terminput::KeyCode::Enter if key_active && ui_focus == UiFocus::Metronome => {
            *metronome_enabled = !*metronome_enabled;
            *last_metronome_beat_emitted = None;
            true
        }
        terminput::KeyCode::Char('-') | terminput::KeyCode::Char('_')
            if ui_focus == UiFocus::Metronome
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            *metronome_bpm = metronome_bpm.saturating_sub(5).clamp(30, 280);
            *last_metronome_beat_emitted = None;
            true
        }
        terminput::KeyCode::Char('=') | terminput::KeyCode::Char('+')
            if ui_focus == UiFocus::Metronome
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            *metronome_bpm = metronome_bpm.saturating_add(5).clamp(30, 280);
            *last_metronome_beat_emitted = None;
            true
        }
        terminput::KeyCode::Char(',')
            if ui_focus == UiFocus::Metronome
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            *metronome_beats_per_bar = metronome_beats_per_bar.saturating_sub(1).clamp(1, 12);
            *last_metronome_beat_emitted = None;
            true
        }
        terminput::KeyCode::Char('.')
            if ui_focus == UiFocus::Metronome
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            *metronome_beats_per_bar = metronome_beats_per_bar.saturating_add(1).clamp(1, 12);
            *last_metronome_beat_emitted = None;
            true
        }
        _ => false,
    }
}

pub enum LoopKeyAction {
    ToggleRecording,
    ToggleSelected,
    DeleteSelected,
    AdjustVolume(i8),
}

pub fn handle_loop_key(
    key_event: &terminput::KeyEvent,
    ui_focus: UiFocus,
) -> Option<LoopKeyAction> {
    match key_event.code {
        terminput::KeyCode::Char('u')
            if matches!(ui_focus, UiFocus::AddLoop)
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            Some(LoopKeyAction::ToggleRecording)
        }
        terminput::KeyCode::Enter
            if key_event.kind != terminput::KeyEventKind::Release
                && matches!(ui_focus, UiFocus::AddLoop | UiFocus::Loop(_)) =>
        {
            if matches!(ui_focus, UiFocus::AddLoop) {
                Some(LoopKeyAction::ToggleRecording)
            } else {
                Some(LoopKeyAction::ToggleSelected)
            }
        }
        terminput::KeyCode::Char('i')
            if matches!(ui_focus, UiFocus::Loop(_))
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            Some(LoopKeyAction::ToggleSelected)
        }
        terminput::KeyCode::Char('o') | terminput::KeyCode::Backspace
            if matches!(ui_focus, UiFocus::Loop(_))
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            Some(LoopKeyAction::DeleteSelected)
        }
        terminput::KeyCode::Char('-') | terminput::KeyCode::Char('_')
            if matches!(ui_focus, UiFocus::Loop(_))
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            Some(LoopKeyAction::AdjustVolume(-5))
        }
        terminput::KeyCode::Char('=') | terminput::KeyCode::Char('+')
            if matches!(ui_focus, UiFocus::Loop(_))
                && matches!(key_event.kind, terminput::KeyEventKind::Press) =>
        {
            Some(LoopKeyAction::AdjustVolume(5))
        }
        _ => None,
    }
}

