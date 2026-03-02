use ratatui::{
    Terminal,
    layout::Rect,
    style::Color,
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
    audio::{AudioWriter, mixer},
    terminal::{TerminalGamesBackend, TerminalReader},
    terminput,
};

mod components;
mod looping;
mod model;
mod piano;
mod ui;

use crate::{
    components::*,
    looping::*,
    model::{LoopEventKind, LoopRecording, LoopTrack, PresetChoice, UiFocus},
    piano::*,
    ui::*,
};

pub const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / 60);
pub const MIXER_TICK_INTERVAL: Duration = Duration::from_millis(5);
pub const LEGACY_NOTE_DURATION: Duration = Duration::from_millis(180);
pub const SUSTAIN_TAIL_DURATION: Duration = Duration::from_millis(1800);
pub const ENHANCED_STUCK_TIMEOUT: Duration = Duration::from_millis(1200);
pub const KITTY_KEYBOARD_ENABLE: &[u8] = b"\x1b[>11u";
pub const KITTY_KEYBOARD_DISABLE: &[u8] = b"\x1b[<u";
pub const MOUSE_ENABLE: &[u8] = b"\x1b[?1002h\x1b[?1006h";
pub const MOUSE_DISABLE: &[u8] = b"\x1b[?1002l\x1b[?1006l";
pub const MIDI_LOW_NOTE: i32 = 0;
pub const MIDI_HIGH_NOTE: i32 = 127;
pub const PIANO_KEY_WIDTH: usize = 4;
pub const PIANO_HEIGHT: usize = 13;
pub const SYNTH_CHANNEL: i32 = 0;
pub const KEYBOARD_BASE_NOTE: i32 = 60;
pub const KEYBOARD_MAX_SEMITONE: i32 = 13;
pub const HELP_HEIGHT: usize = 7;
pub const PICKER_MAX_VISIBLE: usize = 8;
pub const MIN_CONTENT_WIDTH_FOR_PADDING: usize = 24;
pub const MIN_CONTENT_HEIGHT_FOR_PADDING: usize = PIANO_HEIGHT + HELP_HEIGHT + 3;
pub const MAX_UI_COLUMN_WIDTH: usize = 96;
pub const LOOP_MIN_DURATION: Duration = Duration::from_millis(300);
pub const LOOP_CHANNEL_START: i32 = 1;
pub const METRONOME_CHANNEL: i32 = 9;
pub const METRONOME_CLICK_DURATION: Duration = Duration::from_millis(40);

#[used]
static TERMINAL_GAMES_MANIFEST: &[u8] = include_bytes!("../terminal-games.json");

static SOUNDFONT: &[u8] = include_bytes!("../TimGM6mb.sf2");

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

fn select_preset(
    idx: usize,
    current_preset_index: &mut usize,
    synthesizer: &mut rustysynth::Synthesizer,
    presets: &[PresetChoice],
    loop_recording: &mut Option<LoopRecording>,
    active_key_notes: &mut HashMap<char, i32>,
    active_notes: &mut HashSet<i32>,
    legacy_note_off_deadlines: &mut HashMap<i32, Instant>,
    sustained_note_off_deadlines: &mut HashMap<i32, Instant>,
    key_last_seen: &mut HashMap<char, Instant>,
    mouse_note: &mut Option<i32>,
) {
    if idx == *current_preset_index {
        return;
    }
    *current_preset_index = idx;
    flush_recording_held_notes(loop_recording);
    clear_playing_notes(
        synthesizer,
        active_key_notes,
        active_notes,
        legacy_note_off_deadlines,
        sustained_note_off_deadlines,
        key_last_seen,
        mouse_note,
    );
    apply_preset(synthesizer, &presets[*current_preset_index], SYNTH_CHANNEL);
    let preset = &presets[*current_preset_index];
    record_loop_event(
        loop_recording,
        LoopEventKind::SetProgram {
            bank: preset.bank.clamp(0, 127),
            patch: preset.patch.clamp(0, 127),
        },
    );
}

fn toggle_loop_recording(
    loop_recording: &mut Option<LoopRecording>,
    loop_tracks: &mut Vec<LoopTrack>,
    presets: &[PresetChoice],
    current_preset_index: usize,
    metronome_bpm: u32,
    sustain_enabled: bool,
    transport_beats: f64,
    metronome_beats_per_bar: u8,
    synthesizer: &mut rustysynth::Synthesizer,
    ui_focus: &mut UiFocus,
) {
    if loop_recording.is_none() {
        *loop_recording = Some(begin_loop_recording(
            current_preset_index,
            metronome_bpm,
            sustain_enabled,
            presets,
        ));
        return;
    }
    let Some(recording) = loop_recording.take() else {
        return;
    };
    let Some(track) = finish_loop_recording(
        recording,
        presets,
        loop_tracks,
        transport_beats,
        metronome_beats_per_bar,
        LOOP_MIN_DURATION,
        SYNTH_CHANNEL,
        METRONOME_CHANNEL,
        LOOP_CHANNEL_START,
    ) else {
        return;
    };
    if let Some(preset) = presets.get(track.preset_index) {
        apply_preset(synthesizer, preset, track.channel);
    }
    apply_loop_volume(synthesizer, track.channel, track.volume_percent);
    loop_tracks.push(track);
    *ui_focus = UiFocus::Loop(loop_tracks.len().saturating_sub(1));
}

fn toggle_loop_track_playback(
    selected_loop_index: usize,
    loop_tracks: &mut [LoopTrack],
    presets: &[PresetChoice],
    synthesizer: &mut rustysynth::Synthesizer,
    transport_beats: f64,
    metronome_beats_per_bar: u8,
) {
    if let Some(track) = loop_tracks.get_mut(selected_loop_index) {
        if track.enabled {
            track.enabled = false;
            track.pending_start_beat = None;
            silence_loop_track(synthesizer, track);
            return;
        }
        if let Some(preset) = presets.get(track.preset_index) {
            apply_preset(synthesizer, preset, track.channel);
        }
        apply_loop_volume(synthesizer, track.channel, track.volume_percent);
        arm_loop_start(synthesizer, track, transport_beats, metronome_beats_per_bar);
    }
}

fn delete_loop_track(
    selected_loop_index: usize,
    loop_tracks: &mut Vec<LoopTrack>,
    ui_focus: &mut UiFocus,
    synthesizer: &mut rustysynth::Synthesizer,
) {
    if let Some(track) = loop_tracks.get_mut(selected_loop_index) {
        silence_loop_track(synthesizer, track);
    } else {
        return;
    }
    loop_tracks.remove(selected_loop_index);
    *ui_focus = UiFocus::Loop(selected_loop_index).normalize(loop_tracks.len());
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
        apply_preset(&mut synthesizer, preset, SYNTH_CHANNEL);
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
    let mut loop_tracks = Vec::<LoopTrack>::new();
    let mut ui_focus = UiFocus::Instrument;
    let mut loop_recording = None::<LoopRecording>;
    let mut transport_beats = 0.0f64;
    let mut last_transport_tick = Instant::now();
    let mut metronome_enabled = false;
    let mut metronome_bpm = 120u32;
    let mut metronome_beats_per_bar = 4u8;
    let mut last_metronome_beat_emitted = None::<u64>;
    let mut metronome_note_off_deadlines = Vec::<(i32, Instant)>::new();

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
        ui_focus = ui_focus.normalize(loop_tracks.len());
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

                match handle_instrument_key(
                    &key_event,
                    key_active,
                    ui_focus,
                    &mut picker_open,
                    &mut preset_filter,
                    &mut picker_selected,
                    &mut picker_scroll,
                    picker_visible_rows,
                    &presets,
                    current_preset_index,
                ) {
                    InstrumentKeyResult::NotHandled => {}
                    InstrumentKeyResult::Handled => continue,
                    InstrumentKeyResult::SelectPreset(idx) => {
                        select_preset(
                            idx,
                            &mut current_preset_index,
                            &mut synthesizer,
                            &presets,
                            &mut loop_recording,
                            &mut active_key_notes,
                            &mut active_notes,
                            &mut legacy_note_off_deadlines,
                            &mut sustained_note_off_deadlines,
                            &mut key_last_seen,
                            &mut mouse_note,
                        );
                        continue;
                    }
                }

                if key_active && matches!(key_event.code, terminput::KeyCode::Up) {
                    ui_focus = ui_focus.prev(loop_tracks.len());
                    continue;
                }
                if key_active && matches!(key_event.code, terminput::KeyCode::Down) {
                    ui_focus = ui_focus.next(loop_tracks.len());
                    continue;
                }

                if handle_metronome_key(
                    &key_event,
                    key_active,
                    ui_focus,
                    &mut metronome_enabled,
                    &mut metronome_bpm,
                    &mut metronome_beats_per_bar,
                    &mut last_metronome_beat_emitted,
                ) {
                    continue;
                }

                if let Some(action) = handle_loop_key(&key_event, ui_focus) {
                    match action {
                        LoopKeyAction::ToggleRecording => {
                            toggle_loop_recording(
                                &mut loop_recording,
                                &mut loop_tracks,
                                &presets,
                                current_preset_index,
                                metronome_bpm,
                                sustain_enabled,
                                transport_beats,
                                metronome_beats_per_bar,
                                &mut synthesizer,
                                &mut ui_focus,
                            );
                        }
                        LoopKeyAction::ToggleSelected => {
                            let UiFocus::Loop(selected_loop_index) = ui_focus else {
                                continue;
                            };
                            toggle_loop_track_playback(
                                selected_loop_index,
                                &mut loop_tracks,
                                &presets,
                                &mut synthesizer,
                                transport_beats,
                                metronome_beats_per_bar,
                            );
                        }
                        LoopKeyAction::DeleteSelected => {
                            let UiFocus::Loop(selected_loop_index) = ui_focus else {
                                continue;
                            };
                            delete_loop_track(
                                selected_loop_index,
                                &mut loop_tracks,
                                &mut ui_focus,
                                &mut synthesizer,
                            );
                        }
                        LoopKeyAction::AdjustVolume(delta) => {
                            let UiFocus::Loop(selected_loop_index) = ui_focus else {
                                continue;
                            };
                            if let Some(track) = loop_tracks.get_mut(selected_loop_index) {
                                if delta > 0 {
                                    track.volume_percent =
                                        track.volume_percent.saturating_add(delta as u8).min(200);
                                } else {
                                    track.volume_percent =
                                        track.volume_percent.saturating_sub((-delta) as u8);
                                }
                                apply_loop_volume(
                                    &mut synthesizer,
                                    track.channel,
                                    track.volume_percent,
                                );
                            }
                        }
                    }
                    continue;
                }

                match key_event.code {
                    terminput::KeyCode::Char(' ') if key_active => {
                        sustain_enabled = !sustain_enabled;
                        if let Some(recording) = &mut loop_recording {
                            recording.sustain_enabled = sustain_enabled;
                        }
                        record_loop_event(
                            &mut loop_recording,
                            LoopEventKind::SetSustain {
                                enabled: sustain_enabled,
                            },
                        );
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
                                    SUSTAIN_TAIL_DURATION,
                                    SYNTH_CHANNEL,
                                    &mut loop_recording,
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

                        flush_recording_held_notes(&mut loop_recording);
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
                                        live_note_on(
                                            &mut synthesizer,
                                            SYNTH_CHANNEL,
                                            note,
                                            110,
                                            &mut loop_recording,
                                        );
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
                                        SUSTAIN_TAIL_DURATION,
                                        SYNTH_CHANNEL,
                                        &mut loop_recording,
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
                                    live_note_on(
                                        &mut synthesizer,
                                        SYNTH_CHANNEL,
                                        note,
                                        110,
                                        &mut loop_recording,
                                    );
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
                                        SUSTAIN_TAIL_DURATION,
                                        SYNTH_CHANNEL,
                                        &mut loop_recording,
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
                let picker_box_rows = if picker_open && picker_width >= 24 {
                    picker_visible_rows + 4
                } else {
                    0
                };
                let list_top = picker_y + 3;
                let list_bottom = list_top + picker_visible_rows;

                if matches!(
                    mouse.kind,
                    terminput::MouseEventKind::Down(terminput::MouseButton::Left)
                ) && content_row == instrument_line_row
                    && content_col >= ui_left
                    && content_col < ui_left + ui_width
                {
                    ui_focus = UiFocus::Instrument;
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
                            select_preset(
                                idx,
                                &mut current_preset_index,
                                &mut synthesizer,
                                &presets,
                                &mut loop_recording,
                                &mut active_key_notes,
                                &mut active_notes,
                                &mut legacy_note_off_deadlines,
                                &mut sustained_note_off_deadlines,
                                &mut key_last_seen,
                                &mut mouse_note,
                            );
                            picker_open = false;
                        }
                    }
                    continue;
                }

                let metronome_line_row = instrument_line_row + 1 + picker_box_rows;
                let first_loop_row = metronome_line_row + 1;
                let add_loop_row = first_loop_row + loop_tracks.len();

                if matches!(
                    mouse.kind,
                    terminput::MouseEventKind::Down(terminput::MouseButton::Left)
                ) {
                    if content_row == metronome_line_row
                        && content_col >= ui_left
                        && content_col < ui_left + ui_width
                    {
                        ui_focus = UiFocus::Metronome;
                        let rel = content_col.saturating_sub(ui_left);
                        match metronome_mouse_action(
                            rel,
                            metronome_bpm,
                            metronome_beats_per_bar,
                            metronome_enabled,
                        ) {
                            MetronomeMouseAction::None => {}
                            MetronomeMouseAction::ToggleEnabled => {
                                metronome_enabled = !metronome_enabled;
                                last_metronome_beat_emitted = None;
                            }
                            MetronomeMouseAction::AdjustBpm(delta) => {
                                if delta > 0 {
                                    metronome_bpm = metronome_bpm.saturating_add(delta as u32).clamp(30, 280);
                                } else {
                                    metronome_bpm = metronome_bpm.saturating_sub((-delta) as u32).clamp(30, 280);
                                }
                                last_metronome_beat_emitted = None;
                            }
                            MetronomeMouseAction::AdjustBar(delta) => {
                                if delta > 0 {
                                    metronome_beats_per_bar =
                                        metronome_beats_per_bar.saturating_add(delta as u8).clamp(1, 12);
                                } else {
                                    metronome_beats_per_bar =
                                        metronome_beats_per_bar.saturating_sub((-delta) as u8).clamp(1, 12);
                                }
                                last_metronome_beat_emitted = None;
                            }
                        }
                        continue;
                    }

                    if content_row >= first_loop_row && content_row < first_loop_row + loop_tracks.len()
                    {
                        let idx = content_row - first_loop_row;
                        ui_focus = UiFocus::Loop(idx);
                        if let Some(track) = loop_tracks.get(idx) {
                            match loop_row_mouse_action(content_col.saturating_sub(ui_left), track) {
                                LoopRowMouseAction::None => {}
                                LoopRowMouseAction::TogglePlayback => {
                                    toggle_loop_track_playback(
                                        idx,
                                        &mut loop_tracks,
                                        &presets,
                                        &mut synthesizer,
                                        transport_beats,
                                        metronome_beats_per_bar,
                                    );
                                    continue;
                                }
                                LoopRowMouseAction::Delete => {
                                    delete_loop_track(
                                        idx,
                                        &mut loop_tracks,
                                        &mut ui_focus,
                                        &mut synthesizer,
                                    );
                                    continue;
                                }
                            }
                        }
                    }

                    if content_row == add_loop_row
                        && content_col >= ui_left
                        && content_col < ui_left + ui_width
                    {
                        ui_focus = UiFocus::AddLoop;
                        toggle_loop_recording(
                            &mut loop_recording,
                            &mut loop_tracks,
                            &presets,
                            current_preset_index,
                            metronome_bpm,
                            sustain_enabled,
                            transport_beats,
                            metronome_beats_per_bar,
                            &mut synthesizer,
                            &mut ui_focus,
                        );
                        continue;
                    }
                }

                if matches!(
                    mouse.kind,
                    terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up)
                        | terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down)
                ) && content_row >= first_loop_row
                    && content_row < first_loop_row + loop_tracks.len()
                    && content_col >= ui_left
                    && content_col < ui_left + ui_width
                {
                    let idx = content_row - first_loop_row;
                    ui_focus = UiFocus::Loop(idx);
                    if let Some(track) = loop_tracks.get_mut(idx) {
                        match mouse.kind {
                            terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up) => {
                                track.volume_percent = track.volume_percent.saturating_add(5).min(200);
                            }
                            terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down) => {
                                track.volume_percent = track.volume_percent.saturating_sub(5);
                            }
                            _ => {}
                        }
                        apply_loop_volume(&mut synthesizer, track.channel, track.volume_percent);
                    }
                    continue;
                }

                if matches!(
                    mouse.kind,
                    terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up)
                        | terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down)
                ) && content_row == metronome_line_row
                    && content_col >= ui_left
                    && content_col < ui_left + ui_width
                {
                    ui_focus = UiFocus::Metronome;
                    match mouse.kind {
                        terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Up) => {
                            metronome_bpm = metronome_bpm.saturating_add(1).clamp(30, 280);
                        }
                        terminput::MouseEventKind::Scroll(terminput::ScrollDirection::Down) => {
                            metronome_bpm = metronome_bpm.saturating_sub(1).clamp(30, 280);
                        }
                        _ => {}
                    }
                    last_metronome_beat_emitted = None;
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
                                    SUSTAIN_TAIL_DURATION,
                                    SYNTH_CHANNEL,
                                    &mut loop_recording,
                                );
                            }

                            if let Some(new_note) = hit {
                                mouse_note = Some(new_note);
                                sustained_note_off_deadlines.remove(&new_note);
                                active_notes.insert(new_note);
                                live_note_on(
                                    &mut synthesizer,
                                    SYNTH_CHANNEL,
                                    new_note,
                                    110,
                                    &mut loop_recording,
                                );
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
                                SUSTAIN_TAIL_DURATION,
                                SYNTH_CHANNEL,
                                &mut loop_recording,
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
                        SUSTAIN_TAIL_DURATION,
                        SYNTH_CHANNEL,
                        &mut loop_recording,
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
                    SUSTAIN_TAIL_DURATION,
                    SYNTH_CHANNEL,
                    &mut loop_recording,
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
                    SUSTAIN_TAIL_DURATION,
                    SYNTH_CHANNEL,
                    &mut loop_recording,
                );
            }
        }

        let now = Instant::now();
        let delta = now.saturating_duration_since(last_transport_tick);
        last_transport_tick = now;
        let previous_transport_beats = transport_beats;
        transport_beats += delta.as_secs_f64() * (metronome_bpm as f64) / 60.0;
        for track in &mut loop_tracks {
            process_loop_track(
                &mut synthesizer,
                track,
                previous_transport_beats,
                transport_beats,
            );
        }
        if metronome_enabled {
            let current_beat = transport_beats.floor() as u64;
            let first = last_metronome_beat_emitted
                .map(|beat| beat.saturating_add(1))
                .unwrap_or(current_beat.saturating_add(1));
            for beat in first..=current_beat {
                let beat_in_bar = (beat % metronome_beats_per_bar as u64) as u8;
                let (note, velocity) = if beat_in_bar == 0 {
                    (76, 120)
                } else {
                    (77, 90)
                };
                synthesizer.note_on(METRONOME_CHANNEL, note, velocity);
                metronome_note_off_deadlines.push((note, now + METRONOME_CLICK_DURATION));
            }
            last_metronome_beat_emitted = Some(current_beat);
        } else {
            last_metronome_beat_emitted = None;
        }

        metronome_note_off_deadlines.retain(|(note, deadline)| {
            let keep = now < *deadline;
            if !keep {
                synthesizer.note_off(METRONOME_CHANNEL, *note);
            }
            keep
        });

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
            if content.height == 0 || content.width == 0 {
                return;
            }

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
            let pad = " ".repeat(ui_left);
            let picker_width = ui_width.saturating_sub(2).min(58);
            let picker_visible_rows = picker_visible_rows_for_height(content.height as usize);
            let picker_box_rows =
                render_rows_for_layout(content.height as usize, picker_open, picker_width);

            let mut loop_note_colors = HashMap::<i32, Color>::new();
            for (idx, track) in loop_tracks.iter().enumerate() {
                if !track.enabled {
                    continue;
                }
                let color = loop_color(idx);
                for &note in &track.active_notes {
                    loop_note_colors.entry(note).or_insert(color);
                }
            }

            let help_height = 3u16.min(content.height);
            let controls_bottom = content.y + content.height.saturating_sub(help_height);
            let mut y = content.y;

            let piano_height = (PIANO_HEIGHT as u16).min(content.height);
            if piano_height > 0 {
                frame.render_widget(
                    PianoWidget {
                        live_active_notes: &active_notes,
                        loop_note_colors: &loop_note_colors,
                        visible_white_notes,
                        keyboard_base_note,
                        left_pad: piano_left,
                    },
                    Rect {
                        x: content.x,
                        y,
                        width: content.width,
                        height: piano_height,
                    },
                );
                y = y.saturating_add(piano_height);
            }

            if y < controls_bottom {
                y = y.saturating_add(1);
            }

            let instrument = presets
                .get(current_preset_index)
                .map(|preset| {
                    format!(
                        "{} (bank {}, patch {})",
                        preset.name, preset.bank, preset.patch
                    )
                })
                .unwrap_or_else(|| "N/A".to_string());

            if y < controls_bottom {
                frame.render_widget(
                    InstrumentWidget {
                        pad: &pad,
                        instrument: &instrument,
                        focused: ui_focus == UiFocus::Instrument,
                    },
                    Rect {
                        x: content.x,
                        y,
                        width: content.width,
                        height: 1,
                    },
                );
                y = y.saturating_add(1);
            }

            if picker_open && picker_width >= 24 && y < controls_bottom {
                let picker_height = (picker_box_rows as u16).min(controls_bottom.saturating_sub(y));
                if picker_height > 0 {
                    frame.render_widget(
                        PresetPickerWidget {
                            pad: &pad,
                            presets: &presets,
                            preset_filter: &preset_filter,
                            picker_selected,
                            picker_scroll,
                            picker_width,
                            picker_visible_rows,
                        },
                        Rect {
                            x: content.x,
                            y,
                            width: content.width,
                            height: picker_height,
                        },
                    );
                    y = y.saturating_add(picker_height);
                }
            }

            if y < controls_bottom {
                frame.render_widget(
                    MetronomeWidget {
                        pad: &pad,
                        focused: ui_focus == UiFocus::Metronome,
                        bpm: metronome_bpm,
                        beats_per_bar: metronome_beats_per_bar,
                        enabled: metronome_enabled,
                    },
                    Rect {
                        x: content.x,
                        y,
                        width: content.width,
                        height: 1,
                    },
                );
                y = y.saturating_add(1);
            }

            if y < controls_bottom && !loop_tracks.is_empty() {
                let loop_rows = (loop_tracks.len() as u16).min(controls_bottom.saturating_sub(y));
                if loop_rows > 0 {
                    frame.render_widget(
                        LoopListWidget {
                            pad: &pad,
                            ui_focus,
                            loop_tracks: &loop_tracks,
                        },
                        Rect {
                            x: content.x,
                            y,
                            width: content.width,
                            height: loop_rows,
                        },
                    );
                    y = y.saturating_add(loop_rows);
                }
            }

            if y < controls_bottom {
                frame.render_widget(
                    AddLoopWidget {
                        pad: &pad,
                        focused: ui_focus == UiFocus::AddLoop,
                        recording: loop_recording.is_some(),
                    },
                    Rect {
                        x: content.x,
                        y,
                        width: content.width,
                        height: 1,
                    },
                );
            }

            frame.render_widget(
                HelpWidget {
                    pad: &pad,
                    sustain_enabled,
                    enhanced_input,
                },
                Rect {
                    x: content.x,
                    y: content.y + content.height.saturating_sub(help_height),
                    width: content.width,
                    height: help_height,
                },
            );
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
