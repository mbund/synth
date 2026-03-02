use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct PresetChoice {
    pub name: String,
    pub bank: i32,
    pub patch: i32,
}

#[derive(Clone)]
pub enum LoopEventKind {
    NoteOn { note: i32, velocity: i32 },
    NoteOff { note: i32 },
    SetProgram { bank: i32, patch: i32 },
    SetSustain { enabled: bool },
}

#[derive(Clone)]
pub struct LoopEvent {
    pub at: Duration,
    pub kind: LoopEventKind,
}

pub struct LoopRecording {
    pub started_at: Option<Instant>,
    pub events: Vec<LoopEvent>,
    pub held_notes: HashSet<i32>,
    pub bpm: u32,
    pub preset_index: usize,
    pub sustain_enabled: bool,
}

pub struct LoopTrack {
    pub instrument_name: String,
    pub events: Vec<LoopEvent>,
    pub beat_len: u32,
    pub source_bpm: u32,
    pub volume_percent: u8,
    pub enabled: bool,
    pub start_beat: f64,
    pub pending_start_beat: Option<f64>,
    pub active_notes: HashSet<i32>,
    pub channel: i32,
    pub preset_index: usize,
    pub sustain_enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiFocus {
    Instrument,
    Metronome,
    Loop(usize),
    AddLoop,
}

impl UiFocus {
    pub fn prev(self, loop_count: usize) -> Self {
        match self {
            Self::Instrument => Self::AddLoop,
            Self::Metronome => Self::Instrument,
            Self::Loop(0) => Self::Metronome,
            Self::Loop(idx) => Self::Loop(idx.saturating_sub(1)),
            Self::AddLoop => {
                if loop_count == 0 {
                    Self::Metronome
                } else {
                    Self::Loop(loop_count - 1)
                }
            }
        }
    }

    pub fn next(self, loop_count: usize) -> Self {
        match self {
            Self::Instrument => Self::Metronome,
            Self::Metronome => {
                if loop_count == 0 {
                    Self::AddLoop
                } else {
                    Self::Loop(0)
                }
            }
            Self::Loop(idx) => {
                if idx + 1 < loop_count {
                    Self::Loop(idx + 1)
                } else {
                    Self::AddLoop
                }
            }
            Self::AddLoop => Self::Instrument,
        }
    }

    pub fn normalize(self, loop_count: usize) -> Self {
        match self {
            Self::Loop(idx) if idx >= loop_count => {
                if loop_count == 0 {
                    Self::AddLoop
                } else {
                    Self::Loop(loop_count - 1)
                }
            }
            other => other,
        }
    }
}
