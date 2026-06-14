//! Top window chrome: the tabbar and menubar model plus their pixel geometry.
//!
//! Geometry lives here, in one place, so the GPU renderer (which draws the
//! chrome) and the app (which hit-tests mouse clicks against it) compute the
//! exact same rectangles and never drift. The app supplies a [`TopChrome`]
//! describing the tabs and menus; [`layout`] turns it into concrete pixel
//! [`Region`]s, and [`hit_test`] maps a click to a [`ChromeHit`].

// ========================================================================
// Constants
// ========================================================================

/// Tab width, in cells. Tabs are a fixed width; titles are truncated to fit.
const TAB_CELLS: f32 = 18.0;
/// Width of the trailing close (`×`) target inside a tab, in cells.
const CLOSE_CELLS: f32 = 2.0;
/// Width of the new-tab (`+`) button, in cells.
const NEW_TAB_CELLS: f32 = 3.0;
/// Width of the modern hamburger (`☰`) button, in cells.
const HAMBURGER_CELLS: f32 = 3.0;
/// Horizontal padding around a classic menu title, in cells (one each side).
const MENU_TITLE_PAD_CELLS: f32 = 2.0;
/// Minimum dropdown panel width, in cells.
const DROPDOWN_MIN_CELLS: f32 = 22.0;
/// Padding added to the widest `label` + `shortcut` when sizing a dropdown, in
/// cells (leading indent, gap between label and shortcut, trailing margin).
const DROPDOWN_PAD_CELLS: f32 = 4.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Which menubar presentation to draw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuStyle {
    /// A `File / Edit / View / …` row above the tabbar; each title opens its menu.
    Classic,
    /// A single `☰` button on the tabbar that opens one dropdown of commands.
    #[default]
    Modern,
}

/// One tab's label in the tabbar.
#[derive(Clone, Debug)]
pub struct TabLabel {
    pub title: String,
}

/// One dropdown menu: a `title` (shown only in the classic menubar) and its
/// items. Item order matches the app's parallel command list.
#[derive(Clone, Debug)]
pub struct Menu {
    pub items: Vec<MenuItem>,
    pub title: String,
}

/// One selectable command line in a dropdown. Purely presentational; the app
/// maps the same index to a command name.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub shortcut: String,
}

/// The full top-chrome model the app hands the renderer each frame.
#[derive(Clone, Debug)]
pub struct TopChrome {
    pub active_tab: usize,
    pub menu_style: MenuStyle,
    pub menus: Vec<Menu>,
    /// Index into `menus` of the open dropdown, or `None` when closed.
    pub open_menu: Option<usize>,
    /// The highlighted dropdown item (mouse hover), if any.
    pub selected_item: Option<usize>,
    pub tabs: Vec<TabLabel>,
}

/// A pixel rectangle in surface coordinates (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Region {
    pub h: f32,
    pub w: f32,
    pub x: f32,
    pub y: f32,
}

/// Geometry of an open dropdown panel and its item rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DropdownLayout {
    pub item_h: f32,
    pub items: usize,
    pub origin_x: f32,
    pub top: f32,
    pub width: f32,
}

/// Concrete pixel geometry of the whole chrome for one frame.
pub(crate) struct ChromeLayout {
    /// Per-tab close (`×`) targets, parallel to `tabs`.
    pub closes: Vec<Region>,
    pub dropdown: Option<DropdownLayout>,
    /// The modern hamburger button, or `None` in classic style.
    pub hamburger: Option<Region>,
    /// Classic menubar band top (`y`), or `None` in modern style.
    pub menubar_top: Option<f32>,
    /// Classic menu-title targets, parallel to `menus`; empty in modern style.
    pub menu_titles: Vec<Region>,
    pub new_tab: Region,
    /// Top (`y`) of the tabbar row.
    pub tab_row_top: f32,
    pub tabs: Vec<Region>,
}

/// What a click landed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChromeHit {
    CloseTab(usize),
    /// An item in the open dropdown, by index into that menu's `items`.
    DropdownItem(usize),
    Hamburger,
    /// A classic menu title, by index into `menus`.
    MenuTitle(usize),
    NewTab,
    None,
    Tab(usize),
}

// ========================================================================
// Region
// ========================================================================

impl Region {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

// ========================================================================
// Public API
// ========================================================================

/// Number of top cell rows the chrome occupies: the tabbar, plus the menubar row
/// in classic style.
pub fn chrome_rows(style: MenuStyle) -> usize {
    match style {
        MenuStyle::Classic => 2,
        MenuStyle::Modern => 1,
    }
}

/// Map a click at `(x, y)` to the chrome element under it. Dropdown items take
/// priority (they overlay everything below), then per-tab close buttons, tabs,
/// the new-tab button, and finally the menu triggers.
pub fn hit_test(
    chrome: &TopChrome,
    surface_w: f32,
    cell_w: f32,
    cell_h: f32,
    x: f32,
    y: f32,
) -> ChromeHit {
    let layout = layout(chrome, surface_w, cell_w, cell_h);

    if let Some(dropdown) = &layout.dropdown {
        for i in 0..dropdown.items {
            if dropdown_item_region(dropdown, i).contains(x, y) {
                return ChromeHit::DropdownItem(i);
            }
        }
    }

    for (i, region) in layout.closes.iter().enumerate() {
        if region.contains(x, y) {
            return ChromeHit::CloseTab(i);
        }
    }
    for (i, region) in layout.tabs.iter().enumerate() {
        if region.contains(x, y) {
            return ChromeHit::Tab(i);
        }
    }
    if layout.new_tab.contains(x, y) {
        return ChromeHit::NewTab;
    }
    if let Some(hamburger) = &layout.hamburger {
        if hamburger.contains(x, y) {
            return ChromeHit::Hamburger;
        }
    }
    for (i, region) in layout.menu_titles.iter().enumerate() {
        if region.contains(x, y) {
            return ChromeHit::MenuTitle(i);
        }
    }
    ChromeHit::None
}

// ========================================================================
// Layout
// ========================================================================

/// Compute the full pixel geometry of the chrome. Shared by [`hit_test`] and the
/// renderer so both agree on every rectangle.
pub(crate) fn layout(chrome: &TopChrome, surface_w: f32, cw: f32, ch: f32) -> ChromeLayout {
    let classic = chrome.menu_style == MenuStyle::Classic;
    let menubar_top = if classic { Some(0.0) } else { None };
    let tab_row_top = if classic { ch } else { 0.0 };

    let tab_w = TAB_CELLS * cw;
    let mut tabs = Vec::with_capacity(chrome.tabs.len());
    let mut closes = Vec::with_capacity(chrome.tabs.len());
    for i in 0..chrome.tabs.len() {
        let x = i as f32 * tab_w;
        tabs.push(Region {
            h: ch,
            w: tab_w,
            x,
            y: tab_row_top,
        });
        closes.push(Region {
            h: ch,
            w: CLOSE_CELLS * cw,
            x: x + tab_w - CLOSE_CELLS * cw,
            y: tab_row_top,
        });
    }

    let new_tab = Region {
        h: ch,
        w: NEW_TAB_CELLS * cw,
        x: chrome.tabs.len() as f32 * tab_w,
        y: tab_row_top,
    };

    let hamburger = (!classic).then(|| Region {
        h: ch,
        w: HAMBURGER_CELLS * cw,
        x: (surface_w - HAMBURGER_CELLS * cw).max(0.0),
        y: tab_row_top,
    });

    let menu_titles = if classic {
        let mut titles = Vec::with_capacity(chrome.menus.len());
        let mut x = 0.0;
        for menu in &chrome.menus {
            let w = (menu.title.chars().count() as f32 + MENU_TITLE_PAD_CELLS) * cw;
            titles.push(Region {
                h: ch,
                w,
                x,
                y: 0.0,
            });
            x += w;
        }
        titles
    } else {
        Vec::new()
    };

    let dropdown = chrome.open_menu.and_then(|open| {
        let menu = chrome.menus.get(open)?;
        let width = dropdown_width(menu, cw).min(surface_w);
        let (origin_x, top) = if classic {
            let title = menu_titles.get(open)?;
            (title.x, ch)
        } else {
            let hb = hamburger.as_ref()?;
            ((hb.x + hb.w - width).max(0.0), tab_row_top + ch)
        };
        Some(DropdownLayout {
            item_h: ch,
            items: menu.items.len(),
            origin_x,
            top,
            width,
        })
    });

    ChromeLayout {
        closes,
        dropdown,
        hamburger,
        menubar_top,
        menu_titles,
        new_tab,
        tab_row_top,
        tabs,
    }
}

/// The pixel rect of dropdown item `i`.
pub(crate) fn dropdown_item_region(dropdown: &DropdownLayout, i: usize) -> Region {
    Region {
        h: dropdown.item_h,
        w: dropdown.width,
        x: dropdown.origin_x,
        y: dropdown.top + i as f32 * dropdown.item_h,
    }
}

/// Panel width sized to the widest `label` + `shortcut`, floored at a minimum.
fn dropdown_width(menu: &Menu, cw: f32) -> f32 {
    let widest = menu
        .items
        .iter()
        .map(|item| item.label.chars().count() + item.shortcut.chars().count())
        .max()
        .unwrap_or(0) as f32;
    (widest + DROPDOWN_PAD_CELLS).max(DROPDOWN_MIN_CELLS) * cw
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CW: f32 = 10.0;
    const CH: f32 = 20.0;
    const SURFACE_W: f32 = 1200.0;

    fn chrome(style: MenuStyle, tabs: usize, open: Option<usize>) -> TopChrome {
        let menus = match style {
            MenuStyle::Modern => vec![Menu {
                title: "Menu".into(),
                items: vec![
                    MenuItem {
                        label: "New Tab".into(),
                        shortcut: "Ctrl-Shift-T".into(),
                    },
                    MenuItem {
                        label: "Split".into(),
                        shortcut: String::new(),
                    },
                ],
            }],
            MenuStyle::Classic => vec![
                Menu {
                    title: "File".into(),
                    items: vec![MenuItem {
                        label: "New Tab".into(),
                        shortcut: String::new(),
                    }],
                },
                Menu {
                    title: "View".into(),
                    items: vec![MenuItem {
                        label: "Theme".into(),
                        shortcut: String::new(),
                    }],
                },
            ],
        };
        TopChrome {
            active_tab: 0,
            menu_style: style,
            menus,
            open_menu: open,
            selected_item: None,
            tabs: (0..tabs)
                .map(|i| TabLabel {
                    title: format!("Tab {i}"),
                })
                .collect(),
        }
    }

    #[test]
    fn test_chrome_rows_by_style() {
        assert_eq!(chrome_rows(MenuStyle::Modern), 1);
        assert_eq!(chrome_rows(MenuStyle::Classic), 2);
    }

    #[test]
    fn test_modern_tabbar_is_top_row_classic_is_second() {
        let modern = layout(&chrome(MenuStyle::Modern, 2, None), SURFACE_W, CW, CH);
        assert_eq!(modern.tab_row_top, 0.0);
        assert_eq!(modern.menubar_top, None);

        let classic = layout(&chrome(MenuStyle::Classic, 2, None), SURFACE_W, CW, CH);
        assert_eq!(classic.tab_row_top, CH);
        assert_eq!(classic.menubar_top, Some(0.0));
    }

    #[test]
    fn test_hit_test_picks_tab_then_new_tab() {
        let c = chrome(MenuStyle::Modern, 2, None);
        // First tab is at x in [0, 18*cw).
        assert_eq!(hit_test(&c, SURFACE_W, CW, CH, 5.0, 5.0), ChromeHit::Tab(0));
        // Second tab.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, 18.0 * CW + 5.0, 5.0),
            ChromeHit::Tab(1)
        );
        // The new-tab button sits just past the last tab.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, 2.0 * 18.0 * CW + 5.0, 5.0),
            ChromeHit::NewTab
        );
    }

    #[test]
    fn test_close_target_beats_tab_at_right_edge() {
        let c = chrome(MenuStyle::Modern, 1, None);
        // Far right of the first tab is the close target, not the tab body.
        let x = 18.0 * CW - 5.0;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, x, 5.0),
            ChromeHit::CloseTab(0)
        );
    }

    #[test]
    fn test_hamburger_only_in_modern() {
        let modern = chrome(MenuStyle::Modern, 1, None);
        assert_eq!(
            hit_test(&modern, SURFACE_W, CW, CH, SURFACE_W - 5.0, 5.0),
            ChromeHit::Hamburger
        );
        // Classic has menu titles on the top row instead.
        let classic = chrome(MenuStyle::Classic, 1, None);
        assert_eq!(
            hit_test(&classic, SURFACE_W, CW, CH, 2.0, 2.0),
            ChromeHit::MenuTitle(0)
        );
    }

    #[test]
    fn test_open_dropdown_items_are_hit_first() {
        let c = chrome(MenuStyle::Modern, 1, Some(0));
        let layout = layout(&c, SURFACE_W, CW, CH);
        let dropdown = layout.dropdown.expect("dropdown open");
        let first = dropdown_item_region(&dropdown, 0);
        let hit = hit_test(
            &c,
            SURFACE_W,
            CW,
            CH,
            first.x + 2.0,
            first.y + first.h / 2.0,
        );
        assert_eq!(hit, ChromeHit::DropdownItem(0));
        let second = dropdown_item_region(&dropdown, 1);
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, second.x + 2.0, second.y + 2.0),
            ChromeHit::DropdownItem(1)
        );
    }

    #[test]
    fn test_click_in_empty_space_is_none() {
        let c = chrome(MenuStyle::Modern, 1, None);
        // Middle of the tabbar row, past the new-tab button, before the hamburger.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W / 2.0, 5.0),
            ChromeHit::None
        );
    }

    #[test]
    fn test_classic_dropdown_anchors_under_its_title() {
        let c = chrome(MenuStyle::Classic, 1, Some(1));
        let layout = layout(&c, SURFACE_W, CW, CH);
        let dropdown = layout.dropdown.expect("dropdown open");
        // Opens directly below the menubar row.
        assert_eq!(dropdown.top, CH);
        // Left edge aligns with the second menu title.
        assert_eq!(dropdown.origin_x, layout.menu_titles[1].x);
    }
}
