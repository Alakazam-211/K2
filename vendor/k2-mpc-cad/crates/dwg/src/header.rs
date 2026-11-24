//! The header variables (the `AcDb:Header` section) and the measurement system (the
//! `AcDb:Template` section): the settings the drawing-model read uses (`LTSCALE`, `PSLTSCALE`,
//! `FILLMODE`, `MIRRTEXT`, `PDMODE`, `PDSIZE`, `LWDISPLAY`, `CELTSCALE`), the units
//! (`INSUNITS`, `MEASUREMENT`), model space's extents and the next handle.
//!
//! The section: a 16-byte sentinel, its data's size (`RL`; for 2018, and for 2010 and later
//! with maintenance version above 3, a second `RL`), the data, a CRC-16 over the sizes and the
//! data, the sentinel's complement. The data is every header variable in a fixed order, which
//! depends on the version; from 2007 the data begins with its size in bits (`RL`, counted from
//! that field's own start), its texts sit in a string stream at that end, and its handles (all
//! but the next handle) in a handle stream after it. Every variable is read in order, and from
//! 2007 the read must end exactly where the data ends.
//!
//! From the specification's chapters on the header variables and the template section, as
//! facts; our own code.

use k2_mpc_cad_model::error::CadError;

use crate::bits::Bits;
use crate::classes::{string_stream, tv};
use crate::crc::{crc16, CRC16_SEED};
use crate::error::{BAD_CONTAINER, BAD_VALUE, TRUNCATED};
use crate::frame::{cp1252, Ver};

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

/// What the drawing-model read takes from the header.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HeaderVars {
    pub ltscale: f64,
    pub celtscale: f64,
    pub psltscale: bool,
    pub fillmode: bool,
    pub mirrtext: bool,
    pub pdmode: i16,
    pub pdsize: f64,
    pub lwdisplay: bool,
    pub insunits: i16,
    /// Model space's extents.
    pub extmin: [f64; 3],
    pub extmax: [f64; 3],
    pub handseed: u64,
    /// The current layer (`CLAYER`): its handle, 0 for none.
    pub clayer: u64,
    /// The data's bits left after the last variable: none from 2007 (the data states its end);
    /// for 2004, the padding to the section's last byte (fewer than 8).
    pub trailing_bits: usize,
    /// From 2007, the bits its string stream holds past the texts the format names: texts
    /// another writer added, tolerated (a stream short of them refuses the header).
    pub extra_string_bits: usize,
    /// Every variable read, but those each save changes (dates, the next handle, the version
    /// identifier, the extents), as a digest's input.
    pub stable: String,
}

/// The header data's three streams.
struct Rd<'a> {
    ver: Ver,
    d: Bits<'a>,
    strings: Option<Bits<'a>>,
    handles: Option<Bits<'a>>,
}

impl Rd<'_> {
    fn tv(&mut self) -> Result<String, CadError> {
        if self.ver.strings() {
            match self.strings.as_mut() {
                Some(s) => s.tu(),
                None => Ok(String::new()),
            }
        } else {
            Ok(cp1252(&tv(&mut self.d)?))
        }
    }

    /// A handle: from the handle stream from 2007, else inline.
    fn h(&mut self) -> Result<u64, CadError> {
        match self.handles.as_mut() {
            Some(h) => Ok(h.h()?.value),
            None => Ok(self.d.h()?.value),
        }
    }

    /// An object colour (`CMC`): index, true colour, flags, and its names when they follow.
    fn cmc(&mut self) -> Result<(), CadError> {
        self.d.bs()?;
        self.d.bl()?;
        let f = self.d.rc()?;
        if f & 1 != 0 {
            self.tv()?;
        }
        if f & 2 != 0 {
            self.tv()?;
        }
        Ok(())
    }

    fn n<T>(
        &mut self,
        n: usize,
        f: impl Fn(&mut Self) -> Result<T, CadError>,
    ) -> Result<(), CadError> {
        for _ in 0..n {
            f(self)?;
        }
        Ok(())
    }
}

fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_CONTAINER, why, NEXT).at(at)
}

fn le32(b: &[u8], at: usize) -> Result<u32, CadError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                "the header section is shorter than its sizes",
                NEXT,
            )
            .at(at)
        })
}

/// The header variables in `section` (the `AcDb:Header` section, decompressed), for a file of
/// `ver` and `maintenance` version, its CRC-16 checked.
pub fn header(section: &[u8], ver: Ver, maintenance: u8) -> Result<HeaderVars, CadError> {
    let size = le32(section, 16)? as usize;
    let data_at: usize = if ver == Ver::R2018 || ver >= Ver::R2010 && maintenance > 3 {
        24
    } else {
        20
    };
    let data_end = data_at
        .checked_add(size)
        .filter(|&e| e + 2 <= section.len())
        .ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                "the header section is shorter than its size",
                NEXT,
            )
            .at(16)
        })?;
    let stored = u16::from_le_bytes([section[data_end], section[data_end + 1]]);
    let calc = crc16(CRC16_SEED, &section[16..data_end]);
    if calc != stored {
        return Err(bad(
            format!("the header section's CRC-16 is {stored:04X}, its bytes give {calc:04X}"),
            data_end,
        ));
    }
    let mut r = if ver.strings() {
        let bits = le32(section, data_at)? as usize;
        let end = (data_at * 8)
            .checked_add(bits)
            .filter(|&e| e <= data_end * 8)
            .ok_or_else(|| bad("the header data's bit size runs past its end", data_at))?;
        let strings = string_stream(section, end)?;
        let text_end = strings.as_ref().map_or(end.saturating_sub(1), Bits::pos);
        Rd {
            ver,
            d: Bits::span(section, (data_at + 4) * 8, text_end),
            strings,
            handles: Some(Bits::span(section, end, data_end * 8)),
        }
    } else {
        Rd {
            ver,
            d: Bits::span(section, data_at * 8, data_end * 8),
            strings: None,
            handles: None,
        }
    };
    let since = |v: Ver| ver >= v;
    let (r2007, r2010, r2013) = (since(Ver::R2010), since(Ver::R2010), since(Ver::R2013));
    let mut stable = String::new();
    let mut keep = |name: &str, v: &dyn std::fmt::Debug| {
        stable.push_str(&format!("{name}={v:?};"));
    };
    if r2013 {
        r.d.bll()?;
    }
    r.n(4, |r| r.d.bd())?;
    r.n(4, |r| r.tv())?;
    r.n(2, |r| r.d.bl())?;
    // DIMASO, DIMSHO, PLINEGEN, ORTHOMODE, REGENMODE, FILLMODE, QTEXTMODE, PSLTSCALE, LIMCHECK
    let mut flags = [false; 9];
    for f in &mut flags {
        *f = r.d.b()?;
    }
    keep("flags", &flags);
    let (fillmode, psltscale) = (flags[5], flags[7]);
    r.d.b()?;
    // USRTIMER, SKPOLY, ANGDIR, SPLFRAME, MIRRTEXT, WORLDVIEW, TILEMODE, PLIMCHECK, VISRETAIN,
    // DISPSILH, PELLIPSE
    let mut flags2 = [false; 11];
    for f in &mut flags2 {
        *f = r.d.b()?;
    }
    keep("flags2", &flags2);
    let mirrtext = flags2[4];
    // PROXYGRAPHICS, TREEDEPTH, LUNITS, LUPREC, AUNITS, AUPREC, ATTMODE, PDMODE
    let mut shorts = [0u16; 8];
    for s in &mut shorts {
        *s = r.d.bs()?;
    }
    keep("shorts", &shorts);
    let pdmode = shorts[7] as i16;
    r.n(3, |r| r.d.bl())?;
    // USERI1 to 5, SPLINESEGS, SURFU, SURFV, SURFTYPE, SURFTAB1, SURFTAB2, SPLINETYPE,
    // SHADEDGE, SHADEDIF, UNITMODE, MAXACTVP, ISOLINES, CMLJUST, TEXTQLTY
    let mut shorts2 = [0u16; 19];
    for s in &mut shorts2 {
        *s = r.d.bs()?;
    }
    keep("shorts2", &shorts2);
    // LTSCALE, TEXTSIZE, TRACEWID, SKETCHINC, FILLETRAD, THICKNESS, ANGBASE, PDSIZE, PLINEWID,
    // USERR1 to 5, CHAMFERA to D, FACETRES, CMLSCALE, CELTSCALE
    let mut reals = [0f64; 21];
    for x in &mut reals {
        *x = r.d.bd()?;
    }
    keep("reals", &reals);
    let (ltscale, pdsize, celtscale) = (reals[0], reals[7], reals[20]);
    // MENUNAME: inline before 2007, in the string stream from 2007
    if !(r2007 && crate::mutant::active("HeaderMenuNameSkipped")) {
        r.tv()?;
    }
    // the dates, and three longs between them
    r.n(4, |r| r.d.bl())?;
    r.n(3, |r| r.d.bl())?;
    r.n(4, |r| r.d.bl())?;
    r.cmc()?;
    // the next handle: in the data stream, whatever the version
    let handseed = r.d.h()?.value;
    // CLAYER (the current layer), TEXTSTYLE, CELTYPE, (CMATERIAL), DIMSTYLE, CMLSTYLE
    let clayer = r.h()?;
    let clayer = if crate::mutant::active("CurrentLayerUnread") {
        0
    } else {
        clayer
    };
    r.n(if r2007 { 5 } else { 4 }, |r| r.h())?;
    r.d.bd()?;
    // paper space: INSBASE, EXTMIN, EXTMAX, LIMMIN, LIMMAX, ELEVATION, UCSORG, UCSXDIR, UCSYDIR,
    // UCSNAME, PUCSORTHOREF, PUCSORTHOVIEW, PUCSBASE, the six origins
    let space = |r: &mut Rd<'_>| -> Result<([f64; 3], [f64; 3]), CadError> {
        r.d.bd3()?;
        let min = r.d.bd3()?;
        let max = r.d.bd3()?;
        r.d.rd2()?;
        r.d.rd2()?;
        r.d.bd()?;
        r.n(3, |r| r.d.bd3())?;
        r.h()?;
        r.h()?;
        r.d.bs()?;
        r.h()?;
        r.n(6, |r| r.d.bd3())?;
        Ok((min, max))
    };
    space(&mut r)?;
    let (extmin, extmax) = space(&mut r)?;
    // DIMPOST, DIMAPOST; DIMSCALE to DIMTM
    r.n(2, |r| r.tv())?;
    let mut dims = [0f64; 9];
    for x in &mut dims {
        *x = r.d.bd()?;
    }
    keep("dims", &dims);
    if r2007 {
        r.d.bd()?;
        r.d.bd()?;
        r.d.bs()?;
        r.cmc()?;
    }
    r.n(6, |r| r.d.b())?;
    r.n(3, |r| r.d.bs())?;
    if r2007 {
        r.d.bs()?;
    }
    // DIMTXT to DIMGAP
    r.n(8, |r| r.d.bd())?;
    r.d.bd()?;
    r.d.b()?;
    r.d.bs()?;
    r.n(4, |r| r.d.b())?;
    r.n(3, |r| r.cmc())?;
    // DIMADEC to DIMJUST
    r.n(11, |r| r.d.bs())?;
    r.n(2, |r| r.d.b())?;
    r.n(4, |r| r.d.bs())?;
    r.d.b()?;
    r.d.bs()?;
    if r2007 {
        r.d.b()?;
    }
    if r2010 {
        r.d.b()?;
        r.d.bd()?;
        r.tv()?;
        r.d.bd()?;
        r.tv()?;
    }
    // DIMTXSTY, DIMLDRBLK, DIMBLK, DIMBLK1, DIMBLK2; DIMLTYPE, DIMLTEX1, DIMLTEX2
    r.n(5, |r| r.h())?;
    if r2007 {
        r.n(3, |r| r.h())?;
    }
    r.d.bs()?;
    r.d.bs()?;
    // the nine control objects, three dictionaries
    r.n(12, |r| r.h())?;
    r.d.bs()?;
    r.d.bs()?;
    r.n(2, |r| r.tv())?;
    // LAYOUTS, PLOTSETTINGS, PLOTSTYLES, MATERIALS, COLORS, (VISUALSTYLE), (one more)
    r.n(5, |r| r.h())?;
    if r2007 {
        r.h()?;
    }
    if r2013 {
        r.h()?;
    }
    let lflags = r.d.bl()?;
    let insunits = r.d.bs()? as i16;
    let cepsntype = r.d.bs()?;
    if cepsntype == 3 {
        r.h()?;
    }
    // FINGERPRINTGUID, VERSIONGUID
    r.tv()?;
    r.tv()?;
    r.n(6, |r| r.d.rc())?;
    r.n(2, |r| r.d.bs())?;
    r.n(2, |r| r.d.rc())?;
    r.tv()?;
    // paper space, model space, BYLAYER, BYBLOCK, CONTINUOUS
    r.n(5, |r| r.h())?;
    if r2007 {
        r.d.b()?;
        r.n(2, |r| r.d.bl())?;
        r.n(6, |r| r.d.bd())?;
        r.n(2, |r| r.d.rc())?;
        r.n(6, |r| r.d.bd())?;
        r.d.bs()?;
        r.d.rc()?;
        r.n(3, |r| r.d.bd())?;
        r.d.bl()?;
        r.n(4, |r| r.d.rc())?;
        r.d.b()?;
        r.cmc()?;
        r.n(3, |r| r.h())?;
        r.d.rc()?;
        r.d.bd()?;
    }
    keep("lflags", &lflags);
    keep("insunits", &insunits);
    // 2004: four shorts more, the last variables the specification lists; then only the
    // padding to the section's last byte
    if !ver.strings() {
        if !crate::mutant::active("HeaderTailUnread") {
            r.n(4, |r| r.d.bs())?;
        }
        if r.d.left() >= 8 {
            return Err(CadError::new(
                BAD_VALUE,
                format!(
                    "the header's variables end {} bits before their data's end",
                    r.d.left()
                ),
                NEXT,
            ));
        }
    }
    if ver.strings() && r.d.left() != 0 {
        return Err(CadError::new(
            BAD_VALUE,
            format!(
                "the header's variables end {} bits before their data's end",
                r.d.left()
            ),
            NEXT,
        ));
    }
    Ok(HeaderVars {
        ltscale,
        celtscale,
        psltscale,
        fillmode,
        mirrtext,
        pdmode,
        pdsize,
        lwdisplay: lflags & 0x200 == 0,
        insunits,
        extmin,
        extmax,
        handseed,
        clayer,
        trailing_bits: r.d.left(),
        extra_string_bits: r.strings.as_ref().map_or(0, Bits::left),
        stable,
    })
}

/// The measurement system in `section` (the `AcDb:Template` section): its description's
/// length (`RS`) and bytes, then the measurement (`RS`: 0 imperial, 1 metric).
pub fn measurement(section: &[u8]) -> Result<i64, CadError> {
    let short = || CadError::new(TRUNCATED, "the template section is cut short", NEXT);
    let n = section
        .get(0..2)
        .map(|s| usize::from(u16::from_le_bytes([s[0], s[1]])))
        .ok_or_else(short)?;
    section
        .get(2 + n..4 + n)
        .map(|s| i64::from(u16::from_le_bytes([s[0], s[1]])))
        .ok_or_else(short)
}
