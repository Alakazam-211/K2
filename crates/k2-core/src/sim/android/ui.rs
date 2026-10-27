//! `uiautomator dump` XML → the lane-blind [`RawTree`] (M-A21). Bounds are
//! pixels; we divide by the density scale (dpi / 160) to get dp.

use super::super::lane::RawTree;
use super::super::tree::{RawNode, Rect, Role};
use super::xml;

/// Map an Android view class to a role (by its simple name).
pub fn role_for(class: &str, checkable: bool) -> Role {
    let simple = class.rsplit(['.', '$']).next().unwrap_or(class);
    match simple {
        "Button" | "ImageButton" | "FloatingActionButton" | "MaterialButton" | "ExtendedFloatingActionButton" => Role::Button,
        "EditText" | "AutoCompleteTextView" | "TextInputEditText" | "MultiAutoCompleteTextView" | "SearchView" => Role::Textfield,
        "Switch" | "SwitchCompat" | "SwitchMaterial" | "MaterialSwitch" | "ToggleButton" => Role::Switch,
        "CheckBox" | "MaterialCheckBox" => Role::Checkbox,
        "CheckedTextView" if checkable => Role::Checkbox,
        "RadioButton" | "MaterialRadioButton" => Role::Radio,
        "SeekBar" | "Slider" | "RatingBar" => Role::Slider,
        "TextView" | "CheckedTextView" | "MaterialTextView" => Role::Text,
        "ImageView" | "AppCompatImageView" | "ShapeableImageView" => Role::Image,
        "RecyclerView" | "ListView" | "ScrollView" | "NestedScrollView" | "HorizontalScrollView" | "GridView" | "ViewPager" | "ViewPager2" => Role::List,
        "Toolbar" | "ActionBar" | "MaterialToolbar" | "ActionBarContainer" => Role::Toolbar,
        "WebView" => Role::Webview,
        "TabView" | "TabWidget" => Role::Tab,
        "FrameLayout" | "LinearLayout" | "RelativeLayout" | "ConstraintLayout" | "ViewGroup" | "CoordinatorLayout" | "CardView" | "MaterialCardView" | "DrawerLayout" => Role::Group,
        _ if checkable => Role::Checkbox,
        _ => Role::Other,
    }
}

/// `[x1,y1][x2,y2]` → (x1, y1, x2, y2).
pub fn parse_bounds(s: &str) -> Option<(f32, f32, f32, f32)> {
    let nums: Vec<f32> = s.split(|c: char| !(c.is_ascii_digit() || c == '-')).filter(|t| !t.is_empty()).filter_map(|t| t.parse().ok()).collect();
    (nums.len() == 4).then(|| (nums[0], nums[1], nums[2], nums[3]))
}

fn yes(e: &xml::Element, k: &str) -> bool {
    e.attr(k) == Some("true")
}

fn node(e: &xml::Element, scale: f32) -> RawNode {
    let class = e.attr("class").unwrap_or("");
    let checkable = yes(e, "checkable");
    let role = role_for(class, checkable);
    let text = e.attr("text").filter(|s| !s.is_empty());
    let desc = e.attr("content-desc").filter(|s| !s.is_empty());
    let hint = e.attr("hint").filter(|s| !s.is_empty());
    // Text fields: the text is the value and the label is the description
    // or hint. Everything else: the text (or description) is the label.
    let (label, value) = if role == Role::Textfield {
        (desc.or(hint).map(str::to_string), Some(text.unwrap_or("").to_string()))
    } else if role == Role::Slider {
        (desc.map(str::to_string), text.map(str::to_string))
    } else {
        (text.or(desc).map(str::to_string), None)
    };
    let frame = e.attr("bounds").and_then(parse_bounds).map(|(x1, y1, x2, y2)| Rect {
        x: x1 / scale,
        y: y1 / scale,
        w: (x2 - x1).max(0.0) / scale,
        h: (y2 - y1).max(0.0) / scale,
    });
    RawNode {
        role: Some(role),
        native: class.to_string(),
        label,
        value,
        identifier: e.attr("resource-id").filter(|s| !s.is_empty()).map(str::to_string),
        frame,
        clickable: yes(e, "clickable") || yes(e, "long-clickable"),
        focusable: yes(e, "focusable"),
        scrollable: yes(e, "scrollable"),
        enabled: e.attr("enabled") != Some("false"),
        focused: yes(e, "focused"),
        selected: yes(e, "selected"),
        checked: checkable.then(|| yes(e, "checked")),
        secure: yes(e, "password"),
        children: e.children_named("node").map(|c| node(c, scale)).collect(),
    }
}

/// Parse a dump. `scale` = px per dp; `screen_px` = the display size in
/// the current orientation (for the points size).
pub fn parse_dump(dump: &str, scale: f32, screen_px: (u32, u32)) -> Result<RawTree, String> {
    // `uiautomator dump /dev/tty` appends "UI hierchary dumped to: …".
    let start = dump.find("<?xml").or_else(|| dump.find("<hierarchy")).ok_or("no hierarchy in the dump")?;
    let end = dump.rfind("</hierarchy>").map(|i| i + "</hierarchy>".len()).ok_or("the dump has no </hierarchy>")?;
    let root = xml::parse(&dump[start..end])?;
    if root.name != "hierarchy" {
        return Err(format!("unexpected root <{}>", root.name));
    }
    let rotation = root.attr("rotation").and_then(|r| r.parse::<u32>().ok()).unwrap_or(0);
    let (w, h) = if rotation % 2 == 1 { (screen_px.1, screen_px.0) } else { screen_px };
    let roots: Vec<RawNode> = root.children_named("node").map(|n| node(n, scale)).collect();
    // The app on top: the last window's package that isn't the system UI.
    let app_id = root
        .children_named("node")
        .filter_map(|n| n.attr("package"))
        .filter(|p| *p != "com.android.systemui")
        .last()
        .map(str::to_string);
    Ok(RawTree { width_pt: w as f32 / scale, height_pt: h as f32 / scale, app_id, title: None, roots, took_ms: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.acme" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2424]"><node index="0" text="Settings" resource-id="com.acme:id/title" class="android.widget.TextView" package="com.acme" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[42,63][420,147]" /><node index="1" text="a@b.c" hint="Email" resource-id="com.acme:id/email" class="android.widget.EditText" package="com.acme" content-desc="" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="true" scrollable="false" long-clickable="true" password="false" selected="false" bounds="[42,210][1038,357]" /><node index="2" text="hunter2" resource-id="com.acme:id/pw" class="android.widget.EditText" package="com.acme" content-desc="Password" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="true" password="true" selected="false" bounds="[42,400][1038,547]" /><node index="3" text="" resource-id="" class="android.widget.Switch" package="com.acme" content-desc="Notifications" checkable="true" checked="true" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[900,600][1038,700]" /><node index="4" text="Save" resource-id="com.acme:id/save" class="android.widget.Button" package="com.acme" content-desc="" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[798,2100][1038,2226]" /></node></hierarchy>UI hierchary dumped to: /dev/tty"#;

    #[test]
    fn maps_roles_labels_values_and_dp_bounds() {
        let t = parse_dump(DUMP, 2.625, (1080, 2424)).expect("parses");
        assert_eq!(t.app_id.as_deref(), Some("com.acme"));
        assert!((t.width_pt - 411.43).abs() < 0.1, "{}", t.width_pt);
        let kids = &t.roots[0].children;
        assert_eq!(kids.len(), 5);
        assert_eq!((kids[0].role, kids[0].label.as_deref()), (Some(Role::Text), Some("Settings")));
        assert_eq!((kids[1].role, kids[1].label.as_deref(), kids[1].value.as_deref()), (Some(Role::Textfield), Some("Email"), Some("a@b.c")));
        assert!(kids[1].focused);
        assert!(kids[2].secure);
        assert_eq!((kids[3].role, kids[3].checked), (Some(Role::Switch), Some(true)));
        let save = &kids[4];
        assert_eq!((save.role, save.identifier.as_deref()), (Some(Role::Button), Some("com.acme:id/save")));
        let f = save.frame.expect("frame");
        assert!((f.x - 304.0).abs() < 0.1 && (f.w - 91.43).abs() < 0.1, "{f:?}");
        // The shared snapshot never shows a password field's value.
        let snap = crate::sim::tree::Snapshot::build(1, &t);
        let text = snap.render("h", true, None, false);
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("button \"Save\""), "{text}");
    }

    #[test]
    fn landscape_swaps_the_screen_size_and_bad_dumps_fail() {
        let d = DUMP.replace("rotation=\"0\"", "rotation=\"1\"");
        let t = parse_dump(&d, 2.625, (1080, 2424)).expect("parses");
        assert!(t.width_pt > t.height_pt);
        assert!(parse_dump("ERROR: could not get idle state.", 1.0, (1, 1)).is_err());
        assert_eq!(parse_bounds("[0,0][1080,2424]"), Some((0.0, 0.0, 1080.0, 2424.0)));
    }
}
