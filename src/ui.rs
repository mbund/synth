use crate::{model::PresetChoice, HELP_HEIGHT, PICKER_MAX_VISIBLE, PIANO_HEIGHT};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

fn styled_key_spans(key: &str, muted: Style, very_muted: Style) -> Vec<Span<'static>> {
    if key.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    for (idx, segment) in key.split('/').enumerate() {
        if idx > 0 {
            spans.push(Span::styled("/".to_string(), very_muted));
        }
        spans.push(Span::styled(segment.to_string(), muted));
    }
    spans
}

pub fn filter_preset_indices(presets: &[PresetChoice], filter: &str) -> Vec<usize> {
    let needle = filter.trim().to_ascii_lowercase();
    presets
        .iter()
        .enumerate()
        .filter_map(|(idx, preset)| {
            let haystack = format!("{} {} {}", preset.name, preset.bank, preset.patch).to_ascii_lowercase();
            haystack.contains(&needle).then_some(idx)
        })
        .collect()
}

pub fn sync_picker_state(
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

pub fn truncate_and_pad(input: &str, width: usize) -> String {
    let mut out: String = input.chars().take(width).collect();
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

pub fn picker_visible_rows_for_height(total_height: usize) -> usize {
    let reserved = PIANO_HEIGHT + 1 + 1 + HELP_HEIGHT + 4;
    total_height
        .saturating_sub(reserved)
        .max(1)
        .min(PICKER_MAX_VISIBLE)
}

pub fn help_row(
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

    let mut spans = vec![Span::raw(pad.to_string())];
    spans.extend(styled_key_spans(left_key, muted, very_muted));
    spans.push(Span::styled(format!("{left_desc_with_prefix}{left_gap}"), very_muted));
    spans.extend(styled_key_spans(right_key, muted, very_muted));
    spans.push(Span::styled(
        if right_key.is_empty() || right_desc.is_empty() {
            right_desc.to_string()
        } else {
            format!(" {right_desc}")
        },
        very_muted,
    ));
    Line::from(spans)
}
