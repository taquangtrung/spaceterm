//! Native settings page: a keyboard-driven, text-mode overlay that replaces the
//! cross-platform-fragile WebView form. Each row edits one config setting; the
//! mutating methods return the changed `(key, value)` so the app can route it
//! through the same `apply_setting` path the WebView used. Rendering lives in the
//! app's `render` module, which paints this model as a terminal grid, so it works
//! on every platform the GPU renderer does (the WebView did not on Linux).

// ========================================================================
// Data Structures
// ========================================================================

/// The editable control behind one settings row.
#[derive(Clone, Debug)]
pub enum Control {
    Choice(ChoiceControl),
    Number(NumberControl),
    Text(TextControl),
    Toggle(ToggleControl),
}

/// A cycling enum control (theme, menu style): the chosen `index` into `options`.
#[derive(Clone, Debug)]
pub struct ChoiceControl {
    pub index: usize,
    pub options: Vec<ChoiceOption>,
}

/// One option of a [`ChoiceControl`]: the `label` shown to the user and the
/// `value` written to the config. They differ for e.g. "Modern" -> "modern".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChoiceOption {
    pub label: String,
    pub value: String,
}

/// A numeric control adjusted in fixed `step`s and clamped to `[min, max]`.
/// `decimals` controls how the value is formatted for both display and config.
#[derive(Clone, Debug)]
pub struct NumberControl {
    pub decimals: usize,
    pub max: f32,
    pub min: f32,
    pub step: f32,
    pub value: f32,
}

/// One row of the settings page: a config `key`, its human `label`, and the
/// `control` that edits it. `section`, when set, starts a new titled group above
/// this row; `note` is an optional dim one-line description (VSCode-style).
#[derive(Clone, Debug)]
pub struct SettingsField {
    pub control: Control,
    pub key: String,
    pub label: String,
    pub note: Option<String>,
    pub section: Option<String>,
}

/// The open settings page: an ordered list of `fields` and the `selected` row.
#[derive(Clone, Debug)]
pub struct SettingsPage {
    pub fields: Vec<SettingsField>,
    pub selected: usize,
}

/// A free-text control (font family).
#[derive(Clone, Debug)]
pub struct TextControl {
    pub value: String,
}

/// An on/off control (status-bar visibility flags).
#[derive(Clone, Debug)]
pub struct ToggleControl {
    pub on: bool,
}

// ========================================================================
// Control
// ========================================================================

impl Control {
    /// The config value this control serializes to, as `apply_setting` expects it.
    pub fn value(&self) -> String {
        match self {
            Self::Choice(c) => c
                .options
                .get(c.index)
                .map(|o| o.value.clone())
                .unwrap_or_default(),
            Self::Number(n) => format!("{:.*}", n.decimals, n.value),
            Self::Text(t) => t.value.clone(),
            Self::Toggle(t) => if t.on { "true" } else { "false" }.to_string(),
        }
    }
}

// ========================================================================
// SettingsField
// ========================================================================

impl SettingsField {
    pub fn choice(key: &str, label: &str, options: Vec<ChoiceOption>, index: usize) -> Self {
        Self::new(
            key,
            label,
            Control::Choice(ChoiceControl { index, options }),
        )
    }

    pub fn number(
        key: &str,
        label: &str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        decimals: usize,
    ) -> Self {
        Self::new(
            key,
            label,
            Control::Number(NumberControl {
                decimals,
                max,
                min,
                step,
                value,
            }),
        )
    }

    pub fn text(key: &str, label: &str, value: String) -> Self {
        Self::new(key, label, Control::Text(TextControl { value }))
    }

    pub fn toggle(key: &str, label: &str, on: bool) -> Self {
        Self::new(key, label, Control::Toggle(ToggleControl { on }))
    }

    /// Start a new titled section above this row.
    pub fn in_section(mut self, section: &str) -> Self {
        self.section = Some(section.to_string());
        self
    }

    /// Attach a dim one-line description shown under the label.
    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_string());
        self
    }

    fn new(key: &str, label: &str, control: Control) -> Self {
        Self {
            control,
            key: key.to_string(),
            label: label.to_string(),
            note: None,
            section: None,
        }
    }
}

// ========================================================================
// SettingsPage
// ========================================================================

impl SettingsPage {
    pub fn new(fields: Vec<SettingsField>) -> Self {
        Self {
            fields,
            selected: 0,
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.fields.len() {
            self.selected += 1;
        }
    }

    /// Whether the selected row holds a text field, so the caller can route a
    /// space key to typing rather than to value adjustment.
    pub fn selected_is_text(&self) -> bool {
        matches!(
            self.fields.get(self.selected).map(|f| &f.control),
            Some(Control::Text(_))
        )
    }

    /// Step the selected control: cycle a choice, increment/decrement a number,
    /// or flip a toggle (either direction). Returns the changed `(key, value)` to
    /// persist, or `None` if nothing changed (e.g. a text field, or a number at
    /// its bound).
    pub fn adjust(&mut self, forward: bool) -> Option<(String, String)> {
        let field = self.fields.get_mut(self.selected)?;
        match &mut field.control {
            Control::Choice(c) => {
                let n = c.options.len();
                if n <= 1 {
                    return None;
                }
                c.index = if forward {
                    (c.index + 1) % n
                } else {
                    (c.index + n - 1) % n
                };
            }
            Control::Number(num) => {
                let next = if forward {
                    num.value + num.step
                } else {
                    num.value - num.step
                };
                let clamped = next.clamp(num.min, num.max);
                if (clamped - num.value).abs() < f32::EPSILON {
                    return None;
                }
                num.value = clamped;
            }
            Control::Toggle(t) => t.on = !t.on,
            Control::Text(_) => return None,
        }
        Some((field.key.clone(), field.control.value()))
    }

    /// Append a typed character to the selected text field. Returns the changed
    /// `(key, value)`, or `None` if the row is not a text field or `c` is a
    /// control character.
    pub fn push_char(&mut self, c: char) -> Option<(String, String)> {
        if c.is_control() {
            return None;
        }
        let field = self.fields.get_mut(self.selected)?;
        match &mut field.control {
            Control::Text(t) => t.value.push(c),
            _ => return None,
        }
        Some((field.key.clone(), field.control.value()))
    }

    /// Delete the last character of the selected text field. Returns the changed
    /// `(key, value)`, or `None` if the row is not a text field.
    pub fn pop_char(&mut self) -> Option<(String, String)> {
        let field = self.fields.get_mut(self.selected)?;
        match &mut field.control {
            Control::Text(t) => {
                t.value.pop();
            }
            _ => return None,
        }
        Some((field.key.clone(), field.control.value()))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> SettingsPage {
        SettingsPage::new(vec![
            SettingsField::choice(
                "theme",
                "Theme",
                vec![
                    ChoiceOption {
                        label: "Dark".into(),
                        value: "dark".into(),
                    },
                    ChoiceOption {
                        label: "Light".into(),
                        value: "light".into(),
                    },
                ],
                0,
            ),
            SettingsField::toggle("status.enabled", "Show status bar", true),
            SettingsField::text("font_family", "Font family", String::new()),
            SettingsField::number("opacity", "Opacity", 0.9, 0.1, 1.0, 0.05, 2),
        ])
    }

    #[test]
    fn test_choice_cycles_and_wraps() {
        let mut p = page();
        assert_eq!(p.adjust(true), Some(("theme".into(), "light".into())));
        // Wraps back to the first option.
        assert_eq!(p.adjust(true), Some(("theme".into(), "dark".into())));
        // Backward from the first wraps to the last.
        assert_eq!(p.adjust(false), Some(("theme".into(), "light".into())));
    }

    #[test]
    fn test_toggle_flips_both_directions() {
        let mut p = page();
        p.selected = 1;
        assert_eq!(
            p.adjust(true),
            Some(("status.enabled".into(), "false".into()))
        );
        assert_eq!(
            p.adjust(false),
            Some(("status.enabled".into(), "true".into()))
        );
    }

    #[test]
    fn test_number_clamps_at_bounds() {
        let mut p = page();
        p.selected = 3;
        assert_eq!(p.adjust(true), Some(("opacity".into(), "0.95".into())));
        assert_eq!(p.adjust(true), Some(("opacity".into(), "1.00".into())));
        // Already at max: no further change.
        assert_eq!(p.adjust(true), None);
    }

    #[test]
    fn test_text_only_edits_text_fields() {
        let mut p = page();
        // A toggle row ignores typed characters.
        p.selected = 1;
        assert_eq!(p.push_char('x'), None);
        // The text row accepts them.
        p.selected = 2;
        assert_eq!(p.push_char('M'), Some(("font_family".into(), "M".into())));
        assert_eq!(p.push_char('S'), Some(("font_family".into(), "MS".into())));
        assert_eq!(p.pop_char(), Some(("font_family".into(), "M".into())));
        assert!(p.selected_is_text());
    }

    #[test]
    fn test_navigation_clamps_to_field_range() {
        let mut p = page();
        p.move_up();
        assert_eq!(p.selected, 0);
        for _ in 0..10 {
            p.move_down();
        }
        assert_eq!(p.selected, p.fields.len() - 1);
    }
}
