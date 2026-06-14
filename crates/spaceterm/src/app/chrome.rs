//! Top-chrome glue: build the [`TopChrome`] the renderer draws from the app's
//! tab/menu state, and dispatch clicks the renderer's [`hit_test`] resolves.
//!
//! The menu commands reuse the same command names as the command palette (see
//! [`App::run_command`]), so a menu item and a palette entry that do the same
//! thing share one dispatch path.

use spaceterm_render::{ChromeHit, Menu, MenuItem, MenuStyle, TabLabel, TopChrome};

use super::App;

// ========================================================================
// Data Structures
// ========================================================================

/// A dropdown command: its display `label`, an optional `shortcut` hint, and the
/// `command` name dispatched by [`App::run_command`].
struct ItemDef {
    command: &'static str,
    label: &'static str,
    shortcut: &'static str,
}

/// One menu: a `title` (shown only in the classic menubar) and its items.
struct MenuDef {
    items: &'static [ItemDef],
    title: &'static str,
}

// ========================================================================
// Menu definitions
// ========================================================================

const MODERN_ITEMS: &[ItemDef] = &[
    ItemDef {
        command: "new_tab",
        label: "New Tab",
        shortcut: "Ctrl-Shift-T",
    },
    ItemDef {
        command: "close_tab",
        label: "Close Tab",
        shortcut: "Ctrl-Shift-W",
    },
    ItemDef {
        command: "split_vertical",
        label: "Split Vertical",
        shortcut: "",
    },
    ItemDef {
        command: "split_horizontal",
        label: "Split Horizontal",
        shortcut: "",
    },
    ItemDef {
        command: "close_pane",
        label: "Close Pane",
        shortcut: "",
    },
    ItemDef {
        command: "search",
        label: "Search Blocks",
        shortcut: "",
    },
    ItemDef {
        command: "quick_select",
        label: "Quick Select",
        shortcut: "",
    },
    ItemDef {
        command: "toggle_fold",
        label: "Toggle Fold",
        shortcut: "",
    },
    ItemDef {
        command: "theme_dark",
        label: "Theme: Dark",
        shortcut: "",
    },
    ItemDef {
        command: "theme_light",
        label: "Theme: Light",
        shortcut: "",
    },
    ItemDef {
        command: "theme_auto",
        label: "Theme: Auto",
        shortcut: "",
    },
];

const MODERN_MENUS: &[MenuDef] = &[MenuDef {
    items: MODERN_ITEMS,
    title: "Menu",
}];

const CLASSIC_MENUS: &[MenuDef] = &[
    MenuDef {
        title: "File",
        items: &[
            ItemDef {
                command: "new_tab",
                label: "New Tab",
                shortcut: "Ctrl-Shift-T",
            },
            ItemDef {
                command: "close_tab",
                label: "Close Tab",
                shortcut: "Ctrl-Shift-W",
            },
            ItemDef {
                command: "close_pane",
                label: "Close Pane",
                shortcut: "",
            },
        ],
    },
    MenuDef {
        title: "Edit",
        items: &[
            ItemDef {
                command: "search",
                label: "Search Blocks",
                shortcut: "",
            },
            ItemDef {
                command: "quick_select",
                label: "Quick Select",
                shortcut: "",
            },
            ItemDef {
                command: "yank_block",
                label: "Yank Block Source",
                shortcut: "",
            },
        ],
    },
    MenuDef {
        title: "View",
        items: &[
            ItemDef {
                command: "split_vertical",
                label: "Split Vertical",
                shortcut: "",
            },
            ItemDef {
                command: "split_horizontal",
                label: "Split Horizontal",
                shortcut: "",
            },
            ItemDef {
                command: "toggle_fold",
                label: "Toggle Fold",
                shortcut: "",
            },
            ItemDef {
                command: "theme_dark",
                label: "Theme: Dark",
                shortcut: "",
            },
            ItemDef {
                command: "theme_light",
                label: "Theme: Light",
                shortcut: "",
            },
            ItemDef {
                command: "theme_auto",
                label: "Theme: Auto",
                shortcut: "",
            },
        ],
    },
    MenuDef {
        title: "Window",
        items: &[
            ItemDef {
                command: "focus_left",
                label: "Focus Left",
                shortcut: "",
            },
            ItemDef {
                command: "focus_down",
                label: "Focus Down",
                shortcut: "",
            },
            ItemDef {
                command: "focus_up",
                label: "Focus Up",
                shortcut: "",
            },
            ItemDef {
                command: "focus_right",
                label: "Focus Right",
                shortcut: "",
            },
        ],
    },
];

/// The menus for a given style.
fn menu_defs(style: MenuStyle) -> &'static [MenuDef] {
    match style {
        MenuStyle::Classic => CLASSIC_MENUS,
        MenuStyle::Modern => MODERN_MENUS,
    }
}

// ========================================================================
// App — chrome model and interaction
// ========================================================================

impl App {
    /// The tab title from its focused pane, or a `Terminal N` fallback.
    fn tab_title(&self, tab_index: usize) -> String {
        let focused = self.tabs[tab_index].focused();
        match self.pane_titles.get(&focused) {
            Some(title) if !title.is_empty() => title.clone(),
            _ => format!("Terminal {}", tab_index + 1),
        }
    }

    /// Build the chrome model for this frame from the current tab/menu state.
    pub(crate) fn build_top_chrome(&self) -> TopChrome {
        let tabs = (0..self.tabs.len())
            .map(|i| TabLabel {
                title: self.tab_title(i),
            })
            .collect();
        let menus = menu_defs(self.config.menu_style)
            .iter()
            .map(|menu| Menu {
                items: menu
                    .items
                    .iter()
                    .map(|item| MenuItem {
                        label: item.label.to_string(),
                        shortcut: item.shortcut.to_string(),
                    })
                    .collect(),
                title: menu.title.to_string(),
            })
            .collect();
        TopChrome {
            active_tab: self.active_tab,
            menu_style: self.config.menu_style,
            menus,
            open_menu: self.open_menu,
            selected_item: self.selected_item,
            tabs,
        }
    }

    /// Pixel height of the reserved top-chrome band.
    pub(crate) fn top_chrome_height(&self) -> f32 {
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(0.0);
        ch * self.top_chrome_rows() as f32
    }

    /// Handle a left-click at `(x, y)` against the chrome. Returns `true` when the
    /// click was consumed (a chrome element, or dismissing an open menu) and the
    /// caller should not treat it as a pane click.
    pub(crate) fn handle_chrome_click(&mut self, x: f32, y: f32) -> bool {
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return false;
        };
        let surface_w = self.viewport_rect().width;
        let chrome = self.build_top_chrome();
        let hit = spaceterm_render::hit_test(&chrome, surface_w, cw, ch, x, y);
        let menu_was_open = self.open_menu.is_some();
        let focused = self.tab().focused();

        match hit {
            ChromeHit::Tab(i) => {
                self.close_menu();
                self.switch_tab(i);
            }
            ChromeHit::CloseTab(i) => {
                self.close_menu();
                self.close_tab(i);
            }
            ChromeHit::NewTab => {
                self.close_menu();
                self.new_tab();
            }
            ChromeHit::Hamburger => self.toggle_menu(0),
            ChromeHit::MenuTitle(i) => self.toggle_menu(i),
            ChromeHit::DropdownItem(i) => {
                if let Some(open) = self.open_menu {
                    if let Some(command) = menu_defs(self.config.menu_style)
                        .get(open)
                        .and_then(|m| m.items.get(i))
                        .map(|item| item.command)
                    {
                        self.close_menu();
                        self.run_command(command, focused);
                    }
                }
            }
            ChromeHit::None => {
                if menu_was_open {
                    self.close_menu();
                } else {
                    // A click on empty chrome space is still inside the band, so
                    // swallow it rather than letting it select pane text.
                    return y < self.top_chrome_height();
                }
            }
        }
        self.dirty = true;
        true
    }

    /// Update the hovered dropdown item from the pointer position while a menu is
    /// open, requesting a redraw when it changes.
    pub(crate) fn update_menu_hover(&mut self, x: f32, y: f32) {
        if self.open_menu.is_none() {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        let surface_w = self.viewport_rect().width;
        let chrome = self.build_top_chrome();
        let hovered = match spaceterm_render::hit_test(&chrome, surface_w, cw, ch, x, y) {
            ChromeHit::DropdownItem(i) => Some(i),
            _ => None,
        };
        if hovered != self.selected_item {
            self.selected_item = hovered;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Toggle the dropdown for menu `index` open or closed.
    pub(crate) fn toggle_menu(&mut self, index: usize) {
        self.open_menu = if self.open_menu == Some(index) {
            None
        } else {
            Some(index)
        };
        self.selected_item = None;
        self.dirty = true;
    }

    /// Close any open dropdown.
    pub(crate) fn close_menu(&mut self) {
        if self.open_menu.is_some() {
            self.open_menu = None;
            self.selected_item = None;
            self.dirty = true;
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modern_has_one_menu_classic_has_several() {
        assert_eq!(menu_defs(MenuStyle::Modern).len(), 1);
        assert!(menu_defs(MenuStyle::Classic).len() >= 4);
    }

    #[test]
    fn test_every_menu_item_maps_to_a_nonempty_command() {
        for style in [MenuStyle::Modern, MenuStyle::Classic] {
            for menu in menu_defs(style) {
                assert!(!menu.items.is_empty(), "{} has no items", menu.title);
                for item in menu.items {
                    assert!(!item.command.is_empty(), "{} has empty command", item.label);
                }
            }
        }
    }

    #[test]
    fn test_tab_title_falls_back_to_terminal_number() {
        let app = App::new();
        assert_eq!(app.tab_title(0), "Terminal 1");
    }

    #[test]
    fn test_build_top_chrome_reflects_active_tab_and_menu_state() {
        let mut app = App::new();
        app.open_menu = Some(0);
        let chrome = app.build_top_chrome();
        assert_eq!(chrome.tabs.len(), 1);
        assert_eq!(chrome.active_tab, 0);
        assert_eq!(chrome.open_menu, Some(0));
        assert_eq!(chrome.menu_style, MenuStyle::Modern);
    }

    #[test]
    fn test_toggle_menu_opens_then_closes() {
        let mut app = App::new();
        app.toggle_menu(0);
        assert_eq!(app.open_menu, Some(0));
        app.toggle_menu(0);
        assert_eq!(app.open_menu, None);
    }
}
