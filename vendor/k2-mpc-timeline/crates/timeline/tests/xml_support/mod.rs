//! The XML readers' attribute rule, as a row the planted bug `AttrByLocalName` must fail
//! (`tests/mutants.rs`): an attribute is read by its plain name only, never a namespaced
//! attribute that shares its local name. The row runs in `tests/fcpxml.rs` too.

use k2_mpc_timeline::fcpxml::read_fcpxml;
use k2_mpc_timeline::{ReadLimits, Time};

/// A clip whose namespaced attributes (`x:name`, `x:duration`) come before its plain ones.
const NAMESPACED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.10">
  <resources>
    <format id="r1" name="F" frameDuration="1/24s" width="1920" height="1080"/>
    <asset id="r2" name="shot" uid="S1" start="0s" duration="60s" format="r1" src="file:///media/shot.mov"/>
  </resources>
  <library>
    <event name="E" uid="EV1">
      <project name="P" uid="PRJ1">
        <sequence format="r1" duration="10s" tcStart="0s" tcFormat="NDF">
          <spine>
            <asset-clip xmlns:x="urn:example:other" x:name="planted" x:duration="3s" ref="r2" offset="0s" start="0s" duration="10s" name="kept"/>
          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>
"#;

/// The clip reads its plain `name` and `duration`; the namespaced ones are ignored.
pub fn row_attributes_by_plain_name_only() -> Result<(), String> {
    let doc = read_fcpxml(NAMESPACED.as_bytes(), &ReadLimits::default())
        .map_err(|e| format!("the document reads: {e}"))?;
    let tl = doc.timelines.first().ok_or("a timeline")?;
    let item = tl
        .tracks
        .first()
        .and_then(|t| t.items.first())
        .ok_or("a clip on the first track")?;
    if item.name != "kept" {
        return Err(format!(
            "the clip's name is {:?}, not its plain attribute's \"kept\"",
            item.name
        ));
    }
    if item.duration != Time::seconds(10) {
        return Err(format!(
            "the clip's duration is {:?}, not its plain attribute's 10s",
            item.duration
        ));
    }
    Ok(())
}
