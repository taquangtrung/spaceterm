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
/// Height of one dropdown item row as a multiple of the cell height. Taller than
/// a terminal row so the menu reads as an app menu, not a packed text grid.
const DROPDOWN_ITEM_RATIO: f32 = 1.7;
/// Padding above the first and below the last dropdown item, in cells.
const DROPDOWN_PAD_Y_CELLS: f32 = 0.4;

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

/// One selectable line in a dropdown. Purely presentational; the app maps the
/// same index to a command name. An item with `children` is a submenu parent:
/// hovering it opens a child panel to the right instead of running a command.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub children: Vec<MenuItem>,
    pub label: String,
    pub shortcut: String,
}

impl MenuItem {
    /// Whether this item opens a submenu rather than dispatching a command.
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }
}

/// The full top-chrome model the app hands the renderer each frame.
#[derive(Clone, Debug)]
pub struct TopChrome {
    pub active_tab: usize,
    pub menu_style: MenuStyle,
    pub menus: Vec<Menu>,
    /// Index into `menus` of the open dropdown, or `None` when closed.
    pub open_menu: Option<usize>,
    /// Index (into the open menu's `items`) of the parent whose submenu is shown,
    /// or `None` when no submenu is open.
    pub open_submenu: Option<usize>,
    /// The highlighted dropdown item (mouse hover), if any.
    pub selected_item: Option<usize>,
    /// The highlighted submenu child (mouse hover), if any.
    pub selected_subitem: Option<usize>,
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
    /// Vertical padding inside the panel, above the first and below the last row.
    pub pad: f32,
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
    /// The child panel of the open submenu parent, to the right of `dropdown`.
    pub submenu: Option<DropdownLayout>,
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
    /// A child of the open submenu, by index into that submenu's items. The
    /// parent is the chrome's `open_submenu`.
    SubmenuItem(usize),
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

/// Map a click at `(x, y)` to the chrome element under it. The open submenu wins
/// first (it overlays the parent), then the parent dropdown, then per-tab close
/// buttons, tabs, the new-tab button, and finally the menu triggers.
pub fn hit_test(
    chrome: &TopChrome,
    surface_w: f32,
    cell_w: f32,
    cell_h: f32,
    x: f32,
    y: f32,
) -> ChromeHit {
    let layout = layout(chrome, surface_w, cell_w, cell_h);

    if let Some(submenu) = &layout.submenu {
        for i in 0..submenu.items {
            if dropdown_item_region(submenu, i).contains(x, y) {
                return ChromeHit::SubmenuItem(i);
            }
        }
    }
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

    // The modern hamburger sits at the left of the tabbar; the tabs begin after
    // it. Classic style has no hamburger and tabs start at the left edge.
    let hamburger = (!classic).then_some(Region {
        h: ch,
        w: HAMBURGER_CELLS * cw,
        x: 0.0,
        y: tab_row_top,
    });
    let tabs_left = hamburger.map_or(0.0, |hb| hb.w);

    let tab_w = TAB_CELLS * cw;
    let mut tabs = Vec::with_capacity(chrome.tabs.len());
    let mut closes = Vec::with_capacity(chrome.tabs.len());
    for i in 0..chrome.tabs.len() {
        let x = tabs_left + i as f32 * tab_w;
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
        x: tabs_left + chrome.tabs.len() as f32 * tab_w,
        y: tab_row_top,
    };

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
        let width = panel_width(&menu.items, cw).min(surface_w);
        let (origin_x, top) = if classic {
            let title = menu_titles.get(open)?;
            (title.x, ch)
        } else {
            // Left-aligned with the hamburger, opening below the tabbar row.
            let hb = hamburger.as_ref()?;
            (hb.x, tab_row_top + ch)
        };
        Some(DropdownLayout {
            item_h: ch * DROPDOWN_ITEM_RATIO,
            items: menu.items.len(),
            origin_x,
            pad: ch * DROPDOWN_PAD_Y_CELLS,
            top,
            width,
        })
    });

    // The submenu opens to the right of the parent panel, its first child row
    // aligned with the hovered parent item; the parent panel stays put.
    let submenu = dropdown.and_then(|parent| {
        let open = chrome.open_menu?;
        let parent_idx = chrome.open_submenu?;
        let item = chrome.menus.get(open)?.items.get(parent_idx)?;
        if item.children.is_empty() {
            return None;
        }
        Some(DropdownLayout {
            item_h: parent.item_h,
            items: item.children.len(),
            origin_x: parent.origin_x + parent.width,
            pad: parent.pad,
            top: parent.top + parent_idx as f32 * parent.item_h,
            width: panel_width(&item.children, cw).min(surface_w),
        })
    });

    ChromeLayout {
        closes,
        dropdown,
        hamburger,
        menubar_top,
        menu_titles,
        new_tab,
        submenu,
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
        y: dropdown.top + dropdown.pad + i as f32 * dropdown.item_h,
    }
}

/// Panel width sized to the widest `label` + `shortcut`, floored at a minimum.
fn panel_width(items: &[MenuItem], cw: f32) -> f32 {
    let widest = items
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

    fn leaf(label: &str, shortcut: &str) -> MenuItem {
        MenuItem {
            children: Vec::new(),
            label: label.into(),
            shortcut: shortcut.into(),
        }
    }

    fn chrome(style: MenuStyle, tabs: usize, open: Option<usize>) -> TopChrome {
        let menus = match style {
            MenuStyle::Modern => vec![Menu {
                title: "Menu".into(),
                items: vec![
                    leaf("New Tab", "Ctrl-Shift-T"),
                    // A submenu parent: hovering it opens its children.
                    MenuItem {
                        children: vec![leaf("Vertical", ""), leaf("Horizontal", "")],
                        label: "Split".into(),
                        shortcut: String::new(),
                    },
                ],
            }],
            MenuStyle::Classic => vec![
                Menu {
                    title: "File".into(),
                    items: vec![leaf("New Tab", "")],
                },
                Menu {
                    title: "View".into(),
                    items: vec![leaf("Theme", "")],
                },
            ],
        };
        TopChrome {
            active_tab: 0,
            menu_style: style,
            menus,
            open_menu: open,
            open_submenu: None,
            selected_item: None,
            selected_subitem: None,
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
        // The modern hamburger occupies the first 3 cells; tabs follow it.
        let tabs_left = 3.0 * CW;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, tabs_left + 5.0, 5.0),
            ChromeHit::Tab(0)
        );
        // Second tab.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, tabs_left + 18.0 * CW + 5.0, 5.0),
            ChromeHit::Tab(1)
        );
        // The new-tab button sits just past the last tab.
        assert_eq!(
            hit_test(
                &c,
                SURFACE_W,
                CW,
                CH,
                tabs_left + 2.0 * 18.0 * CW + 5.0,
                5.0
            ),
            ChromeHit::NewTab
        );
    }

    #[test]
    fn test_close_target_beats_tab_at_right_edge() {
        let c = chrome(MenuStyle::Modern, 1, None);
        // Far right of the first tab (past the hamburger offset) is the close
        // target, not the tab body.
        let x = 3.0 * CW + 18.0 * CW - 5.0;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, x, 5.0),
            ChromeHit::CloseTab(0)
        );
    }

    #[test]
    fn test_hamburger_only_in_modern() {
        let modern = chrome(MenuStyle::Modern, 1, None);
        // The hamburger now sits at the left edge.
        assert_eq!(
            hit_test(&modern, SURFACE_W, CW, CH, 5.0, 5.0),
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
    fn test_submenu_opens_right_of_parent_and_is_hit_first() {
        let mut c = chrome(MenuStyle::Modern, 1, Some(0));
        c.open_submenu = Some(1); // "Split" carries children.
        let layout = layout(&c, SURFACE_W, CW, CH);
        let parent = layout.dropdown.expect("parent open");
        let submenu = layout.submenu.expect("submenu open");
        // The child panel sits immediately right of the parent panel.
        assert_eq!(submenu.origin_x, parent.origin_x + parent.width);
        assert_eq!(submenu.items, 2);
        // A click on a child resolves to the submenu, which overlays the parent.
        let child = dropdown_item_region(&submenu, 0);
        assert_eq!(
            hit_test(
                &c,
                SURFACE_W,
                CW,
                CH,
                child.x + 2.0,
                child.y + child.h / 2.0
            ),
            ChromeHit::SubmenuItem(0)
        );
    }

    #[test]
    fn test_no_submenu_without_an_open_parent() {
        // open_submenu set but no menu open: no submenu geometry.
        let mut c = chrome(MenuStyle::Modern, 1, None);
        c.open_submenu = Some(1);
        assert!(layout(&c, SURFACE_W, CW, CH).submenu.is_none());
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
