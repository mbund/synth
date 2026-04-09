use crate::model::{LoopEvent, LoopEventKind, LoopRecording, LoopTrack, PresetChoice};
use ratatui::style::Color;
use std::{
    collections::{HashMap, HashSet},
    io,
    time::{Duration, Instant},
};
use terminal_games_sdk::audio::{self, AudioWriter};

pub fn beat_duration_for_bpm(bpm: u32) -> Duration {
    Duration::from_secs_f64(60.0 / bpm as f64)
}

pub fn quantize_loop_beats(length: Duration, bpm: u32) -> u32 {
    let beat = beat_duration_for_bpm(bpm);
    let beats = (length.as_secs_f64() / beat.as_secs_f64()).ceil();
    beats.max(1.0) as u32
}

pub fn next_bar_start_beat(current_beats: f64, beats_per_bar: u8) -> f64 {
    let bar = (beats_per_bar.max(1)) as f64;
    (current_beats / bar).ceil() * bar
}

pub fn allocate_loop_channel(
    loop_tracks: &[LoopTrack],
    synth_channel: i32,
    metronome_channel: i32,
    fallback_channel: i32,
) -> i32 {
    for channel in 0..16 {
        if channel == synth_channel || channel == metronome_channel {
            continue;
        }
        if !loop_tracks.iter().any(|track| track.channel == channel) {
            return channel;
        }
    }
    fallback_channel
}

pub fn loop_color(index: usize) -> Color {
    const COLORS: [Color; 8] = [
        Color::Rgb(95, 165, 245),
        Color::Rgb(240, 95, 95),
        Color::Rgb(120, 200, 120),
        Color::Rgb(245, 190, 95),
        Color::Rgb(200, 120, 235),
        Color::Rgb(90, 210, 200),
        Color::Rgb(245, 145, 220),
        Color::Rgb(180, 180, 100),
    ];
    COLORS[index % COLORS.len()]
}

pub fn record_loop_event(recording: &mut Option<LoopRecording>, kind: LoopEventKind) {
    if let Some(recording) = recording {
        let at = match recording.started_at {
            Some(started_at) => started_at.elapsed(),
            None => match &kind {
                LoopEventKind::NoteOn { .. } => {
                    recording.started_at = Some(Instant::now());
                    Duration::ZERO
                }
                LoopEventKind::SetProgram { .. } | LoopEventKind::SetSustain { .. } => Duration::ZERO,
                LoopEventKind::NoteOff { .. } => return,
            },
        };
        recording.events.push(LoopEvent { at, kind });
    }
}

pub fn live_note_on(
    synthesizer: &mut rustysynth::Synthesizer,
    synth_channel: i32,
    note: i32,
    velocity: i32,
    recording: &mut Option<LoopRecording>,
) {
    synthesizer.note_on(synth_channel, note, velocity);
    if let Some(recording) = recording {
        recording.held_notes.insert(note);
    }
    record_loop_event(recording, LoopEventKind::NoteOn { note, velocity });
}

pub fn live_note_off(
    synthesizer: &mut rustysynth::Synthesizer,
    synth_channel: i32,
    note: i32,
    recording: &mut Option<LoopRecording>,
) {
    synthesizer.note_off(synth_channel, note);
    if let Some(recording) = recording {
        recording.held_notes.remove(&note);
    }
    record_loop_event(recording, LoopEventKind::NoteOff { note });
}

pub fn flush_recording_held_notes(recording: &mut Option<LoopRecording>) {
    if let Some(recording) = recording {
        let Some(started_at) = recording.started_at else {
            recording.held_notes.clear();
            return;
        };
        let at = started_at.elapsed();
        for note in recording.held_notes.drain() {
            recording.events.push(LoopEvent {
                at,
                kind: LoopEventKind::NoteOff { note },
            });
        }
    }
}

pub fn silence_loop_track(synthesizer: &mut rustysynth::Synthesizer, track: &mut LoopTrack) {
    for note in track.active_notes.drain() {
        synthesizer.note_off(track.channel, note);
    }
}

pub fn arm_loop_start(
    synthesizer: &mut rustysynth::Synthesizer,
    track: &mut LoopTrack,
    current_beats: f64,
    beats_per_bar: u8,
) {
    silence_loop_track(synthesizer, track);
    track.enabled = true;
    track.pending_start_beat = Some(next_bar_start_beat(current_beats, beats_per_bar));
}

pub fn process_loop_track(
    synthesizer: &mut rustysynth::Synthesizer,
    track: &mut LoopTrack,
    previous_beats: f64,
    current_beats: f64,
) {
    if !track.enabled || track.events.is_empty() || track.beat_len == 0 {
        return;
    }
    if let Some(start_at) = track.pending_start_beat {
        if current_beats < start_at {
            return;
        }
        track.start_beat = start_at;
        track.pending_start_beat = None;
    }
    if current_beats < track.start_beat {
        return;
    }

    let adjusted_previous = previous_beats.max(track.start_beat);
    let cycle_beats = track.beat_len as f64;
    let previous_phase = (adjusted_previous - track.start_beat).rem_euclid(cycle_beats);
    let current_phase = (current_beats - track.start_beat).rem_euclid(cycle_beats);
    let source_beat_secs = beat_duration_for_bpm(track.source_bpm).as_secs_f64();

    let mut fire_event = |event: &LoopEvent| match &event.kind {
        LoopEventKind::NoteOn { note, velocity } => {
            synthesizer.note_on(track.channel, *note, *velocity);
            track.active_notes.insert(*note);
        }
        LoopEventKind::NoteOff { note } => {
            synthesizer.note_off(track.channel, *note);
            track.active_notes.remove(note);
        }
        LoopEventKind::SetProgram { bank, patch } => {
            synthesizer.process_midi_message(track.channel, 0xB0, 0x00, *bank);
            synthesizer.process_midi_message(track.channel, 0xC0, *patch, 0);
        }
        LoopEventKind::SetSustain { enabled } => {
            track.sustain_enabled = *enabled;
        }
    };

    let mut process_window = |start: f64, end: f64, include_start: bool| {
        for event in &track.events {
            let event_phase = (event.at.as_secs_f64() / source_beat_secs).rem_euclid(cycle_beats);
            let past_start = if include_start {
                event_phase >= start
            } else {
                event_phase > start
            };
            if past_start && event_phase <= end {
                fire_event(event);
            }
        }
    };

    let include_start = (adjusted_previous - track.start_beat).abs() < 1e-9;
    if current_phase >= previous_phase {
        process_window(previous_phase, current_phase, include_start);
    } else {
        process_window(previous_phase, cycle_beats, include_start);
        process_window(0.0, current_phase, true);
    }
}

pub fn apply_preset(synthesizer: &mut rustysynth::Synthesizer, preset: &PresetChoice, channel: i32) {
    let bank = preset.bank.clamp(0, 127);
    let patch = preset.patch.clamp(0, 127);
    synthesizer.process_midi_message(channel, 0xB0, 0x00, bank);
    synthesizer.process_midi_message(channel, 0xC0, patch, 0);
}

pub fn apply_loop_volume(synthesizer: &mut rustysynth::Synthesizer, channel: i32, volume_percent: u8) {
    let cc_value = ((volume_percent as u16 * 127) / 100).min(127) as i32;
    synthesizer.process_midi_message(channel, 0xB0, 0x07, cc_value);
}

pub fn clear_playing_notes(
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

pub fn release_or_sustain_note(
    note: i32,
    sustain_enabled: bool,
    synthesizer: &mut rustysynth::Synthesizer,
    active_key_notes: &HashMap<char, i32>,
    active_notes: &mut HashSet<i32>,
    legacy_note_off_deadlines: &HashMap<i32, Instant>,
    sustained_note_off_deadlines: &mut HashMap<i32, Instant>,
    mouse_note: Option<i32>,
    sustain_tail_duration: Duration,
    synth_channel: i32,
    recording: &mut Option<LoopRecording>,
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
        sustained_note_off_deadlines.insert(note, Instant::now() + sustain_tail_duration);
        active_notes.insert(note);
    } else if active_notes.remove(&note) {
        live_note_off(synthesizer, synth_channel, note, recording);
    }
}

pub fn begin_loop_recording(
    current_preset_index: usize,
    metronome_bpm: u32,
    sustain_enabled: bool,
    presets: &[PresetChoice],
) -> LoopRecording {
    let mut recording = LoopRecording {
        started_at: None,
        events: Vec::new(),
        held_notes: HashSet::new(),
        bpm: metronome_bpm,
        preset_index: current_preset_index,
        sustain_enabled,
    };
    if let Some(preset) = presets.get(current_preset_index) {
        recording.events.push(LoopEvent {
            at: Duration::ZERO,
            kind: LoopEventKind::SetProgram {
                bank: preset.bank.clamp(0, 127),
                patch: preset.patch.clamp(0, 127),
            },
        });
    }
    recording.events.push(LoopEvent {
        at: Duration::ZERO,
        kind: LoopEventKind::SetSustain {
            enabled: sustain_enabled,
        },
    });
    recording
}

pub fn finish_loop_recording(
    mut recording: LoopRecording,
    presets: &[PresetChoice],
    loop_tracks: &[LoopTrack],
    transport_beats: f64,
    metronome_beats_per_bar: u8,
    loop_min_duration: Duration,
    synth_channel: i32,
    metronome_channel: i32,
    fallback_loop_channel: i32,
) -> Option<LoopTrack> {
    let started_at = recording.started_at?;
    let length = started_at.elapsed().max(loop_min_duration);
    let beat_len = quantize_loop_beats(length, recording.bpm);
    for note in recording.held_notes.drain() {
        recording.events.push(LoopEvent {
            at: length,
            kind: LoopEventKind::NoteOff { note },
        });
    }
    recording.events.sort_by_key(|event| event.at);
    if recording.events.is_empty() {
        return None;
    }

    let channel = allocate_loop_channel(
        loop_tracks,
        synth_channel,
        metronome_channel,
        fallback_loop_channel,
    );
    let instrument_name = presets
        .get(recording.preset_index)
        .map(|preset| preset.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());
    Some(LoopTrack {
        instrument_name,
        events: recording.events,
        beat_len,
        source_bpm: recording.bpm,
        volume_percent: 100,
        enabled: true,
        start_beat: 0.0,
        pending_start_beat: Some(next_bar_start_beat(transport_beats, metronome_beats_per_bar)),
        active_notes: HashSet::new(),
        channel,
        preset_index: recording.preset_index,
        sustain_enabled: recording.sustain_enabled,
    })
}

pub fn pump_audio(
    audio_writer: &mut AudioWriter,
    synthesizer: &mut rustysynth::Synthesizer,
    left: &mut Vec<f32>,
    right: &mut Vec<f32>,
    interleaved: &mut Vec<f32>,
) -> io::Result<()> {
    let needed_frames = audio_writer.should_write()?;
    if needed_frames == 0 {
        return Ok(());
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

    let written = audio::write(interleaved)?;
    if written > 0 {
        audio_writer.next_pts += written as u64;
    }

    Ok(())
}
