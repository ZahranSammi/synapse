use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, ElementId, FocusHandle, FontWeight, KeyDownEvent,
    MouseButton, MouseDownEvent, Rgba, SharedString, Window,
};

use crate::theme::ThemePalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeTone {
    Neutral,
    Accent,
    Success,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKeyOutcome {
    Ignored,
    Changed,
    Submit,
    Escape,
}

/// Apply a single keystroke to an editable string buffer.
/// Extracted as a pure helper so text input behavior can be unit tested without a window.
pub fn apply_keystroke_to_input(
    buf: &mut String,
    key: &str,
    key_char: Option<&str>,
) -> InputKeyOutcome {
    match key {
        "backspace" => {
            if buf.pop().is_some() {
                InputKeyOutcome::Changed
            } else {
                InputKeyOutcome::Ignored
            }
        }
        "enter" => InputKeyOutcome::Submit,
        "escape" => InputKeyOutcome::Escape,
        _ => {
            if let Some(ch) = key_char {
                let filtered: String = ch.chars().filter(|c| !c.is_control()).collect();
                if !filtered.is_empty() {
                    buf.push_str(&filtered);
                    return InputKeyOutcome::Changed;
                }
            }
            InputKeyOutcome::Ignored
        }
    }
}

pub fn badge(label: impl Into<SharedString>, tone: BadgeTone, palette: &ThemePalette) -> AnyElement {
    let (bg, fg, border) = match tone {
        BadgeTone::Neutral => (palette.bg_surface_alt, palette.text_secondary, palette.border),
        BadgeTone::Accent => (palette.accent_soft, palette.accent, palette.accent),
        BadgeTone::Success => (palette.success_bg, palette.success, palette.success),
        BadgeTone::Warning => (palette.warning_bg, palette.warning, palette.warning),
    };

    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label.into())
        .into_any_element()
}

pub fn pill_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    variant: ButtonVariant,
    disabled: bool,
    palette: &ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let (bg, hover_bg, fg, border): (Rgba, Rgba, Rgba, Rgba) = match variant {
        ButtonVariant::Primary => (palette.accent, palette.accent_hover, palette.accent_fg, palette.accent),
        ButtonVariant::Secondary => (palette.bg_surface_alt, palette.bg_hover, palette.text_primary, palette.border),
        ButtonVariant::Danger => (palette.danger_bg, palette.danger, palette.danger, palette.danger),
        ButtonVariant::Ghost => (palette.bg_surface, palette.bg_hover, palette.text_secondary, palette.border_subtle),
    };

    let mut el = div()
        .id(id)
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .child(label.into());

    if disabled {
        el = el.opacity(0.45);
    } else {
        let hover_fg = match variant {
            ButtonVariant::Danger => palette.accent_fg,
            _ => fg,
        };
        el = el
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
            .on_click(on_click);
    }

    el.into_any_element()
}

pub fn section_card(palette: &ThemePalette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(16.0))
        .rounded(px(12.0))
        .bg(palette.bg_surface)
        .border_1()
        .border_color(palette.border)
}

pub fn text_input_box(
    id: impl Into<ElementId>,
    value: &str,
    placeholder: &'static str,
    focus_handle: &FocusHandle,
    window: &Window,
    palette: &ThemePalette,
    on_focus: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    on_key: impl Fn(&KeyDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let is_focused = focus_handle.is_focused(window);
    let border_col = if is_focused {
        palette.border_focus
    } else {
        palette.border
    };
    let display_text: SharedString = if value.is_empty() {
        placeholder.into()
    } else if is_focused {
        format!("{value}|").into()
    } else {
        value.to_string().into()
    };
    let text_col = if value.is_empty() {
        palette.text_muted
    } else {
        palette.text_primary
    };

    div()
        .id(id)
        .track_focus(focus_handle)
        .flex_1()
        .min_w(px(160.0))
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(palette.bg_input)
        .border_1()
        .border_color(border_col)
        .text_sm()
        .text_color(text_col)
        .cursor_text()
        .on_mouse_down(MouseButton::Left, on_focus)
        .on_key_down(on_key)
        .child(display_text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_keystroke_to_input_typing_and_backspace() {
        let mut buf = String::new();
        assert_eq!(
            apply_keystroke_to_input(&mut buf, "2", Some("2")),
            InputKeyOutcome::Changed
        );
        assert_eq!(
            apply_keystroke_to_input(&mut buf, "2", Some("2")),
            InputKeyOutcome::Changed
        );
        assert_eq!(buf, "22");

        assert_eq!(
            apply_keystroke_to_input(&mut buf, "backspace", None),
            InputKeyOutcome::Changed
        );
        assert_eq!(buf, "2");

        assert_eq!(
            apply_keystroke_to_input(&mut buf, "backspace", None),
            InputKeyOutcome::Changed
        );
        assert_eq!(
            apply_keystroke_to_input(&mut buf, "backspace", None),
            InputKeyOutcome::Ignored
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn test_apply_keystroke_to_input_submit_escape_and_control_filter() {
        let mut buf = String::from("lts");
        assert_eq!(
            apply_keystroke_to_input(&mut buf, "enter", Some("\n")),
            InputKeyOutcome::Submit
        );
        assert_eq!(
            apply_keystroke_to_input(&mut buf, "escape", None),
            InputKeyOutcome::Escape
        );
        assert_eq!(
            apply_keystroke_to_input(&mut buf, "tab", Some("\t")),
            InputKeyOutcome::Ignored
        );
        assert_eq!(buf, "lts");
    }
}
