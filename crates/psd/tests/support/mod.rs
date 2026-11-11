//! Hand-built files, for cases the generator never writes: every field is under the test's control.
#![allow(dead_code)]

/// Big-endian helpers, and the pieces of a file.
pub fn be16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}
pub fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}
pub fn be64(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

/// A complete, valid file around the given resources and layer-and-mask section: a 1×1 grayscale
/// image with one channel, so the full reader can compare it too.
pub fn hand_file(psb: bool, depth: u16, resources: &[u8], lm: &[u8]) -> Vec<u8> {
    let mut f = b"8BPS".to_vec();
    f.extend(be16(if psb { 2 } else { 1 }));
    f.extend([0u8; 6]);
    f.extend(be16(1));
    f.extend(be32(1));
    f.extend(be32(1));
    f.extend(be16(depth));
    f.extend(be16(1));
    f.extend(be32(0));
    f.extend(be32(resources.len() as u32));
    f.extend_from_slice(resources);
    if psb {
        f.extend(be64(lm.len() as u64));
    } else {
        f.extend(be32(lm.len() as u32));
    }
    f.extend_from_slice(lm);
    f.extend([0u8, 0]); // raw merged image
    f.extend(vec![0u8; (depth / 8).max(1) as usize]);
    f
}

/// One layer record: a zero rectangle, its channels `(id, stored length)`, its extra data.
pub fn record(psb: bool, channels: &[(i16, u64)], extra: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; 16];
    r.extend(be16(channels.len() as u16));
    for &(id, len) in channels {
        r.extend(id.to_be_bytes());
        if psb {
            r.extend(be64(len));
        } else {
            r.extend(be32(len as u32));
        }
    }
    r.extend_from_slice(b"8BIMnorm");
    r.extend([255u8, 0, 0, 0]);
    r.extend(be32(extra.len() as u32));
    r.extend_from_slice(extra);
    r
}

/// Extra data with no mask, no ranges, a Pascal name (padded to four) and tagged blocks.
pub fn extra(name: &[u8], blocks: &[u8]) -> Vec<u8> {
    let mut e = vec![0u8; 8];
    e.push(name.len() as u8);
    e.extend_from_slice(name);
    while !e.len().is_multiple_of(4) {
        e.push(0);
    }
    e.extend_from_slice(blocks);
    e
}

/// A layer info body: the count, the records, each channel's stored bytes.
pub fn layer_info(count: i16, records: &[Vec<u8>], channels: &[Vec<u8>]) -> Vec<u8> {
    let mut li = count.to_be_bytes().to_vec();
    for r in records {
        li.extend_from_slice(r);
    }
    for c in channels {
        li.extend_from_slice(c);
    }
    if li.len() % 2 == 1 {
        li.push(0);
    }
    li
}

/// One layer named `name` with `channels` single raw channels, each holding `payload`.
pub fn layer_info_one(psb: bool, name: &[u8], channels: i16, payload: u8) -> Vec<u8> {
    let ids: Vec<(i16, u64)> = (0..channels).map(|c| (c, 3)).collect();
    let chans: Vec<Vec<u8>> = (0..channels).map(|_| vec![0u8, 0, payload]).collect();
    layer_info(1, &[record(psb, &ids, &extra(name, &[]))], &chans)
}

/// The section body: layer info (length field per version), global mask, global blocks.
pub fn section(psb: bool, li: &[u8], global_mask: &[u8], blocks: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    if psb {
        s.extend(be64(li.len() as u64));
    } else {
        s.extend(be32(li.len() as u32));
    }
    s.extend_from_slice(li);
    s.extend(be32(global_mask.len() as u32));
    s.extend_from_slice(global_mask);
    s.extend_from_slice(blocks);
    s
}

/// A global block with a 4-byte length: signature, key, data, `padding` bytes after the data.
pub fn block(key: &[u8; 4], data: &[u8], padding: &[u8]) -> Vec<u8> {
    let mut b = b"8BIM".to_vec();
    b.extend_from_slice(key);
    b.extend(be32(data.len() as u32));
    b.extend_from_slice(data);
    b.extend_from_slice(padding);
    b
}

/// A global block with an 8-byte length (the PSB long keys).
pub fn long_block(key: &[u8; 4], data: &[u8], padding: &[u8]) -> Vec<u8> {
    let mut b = b"8B64".to_vec();
    b.extend_from_slice(key);
    b.extend(be64(data.len() as u64));
    b.extend_from_slice(data);
    b.extend_from_slice(padding);
    b
}

/// An image resource: id, Pascal name (padded to even), data (padded to even).
pub fn resource(id: u16, name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut r = b"8BIM".to_vec();
    r.extend(be16(id));
    r.push(name.len() as u8);
    r.extend_from_slice(name);
    if !(1 + name.len()).is_multiple_of(2) {
        r.push(0);
    }
    r.extend(be32(data.len() as u32));
    r.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        r.push(0);
    }
    r
}

/// One layer record for [`psd_file`], with every field the pack reads under the test's control.
#[derive(Debug, Clone)]
pub struct Lay {
    pub name: String,
    /// `top, left, bottom, right`.
    pub rect: [i32; 4],
    pub opacity: u8,
    pub clipping: u8,
    pub flags: u8,
    /// Tagged blocks, in order; each block's data is padded to even.
    pub blocks: Vec<([u8; 4], Vec<u8>)>,
    /// Channels: id and the raw bytes after the compression field.
    pub channels: Vec<(i16, Vec<u8>)>,
}

impl Lay {
    /// A visible 1x1 layer with one raw channel holding `[7]`, its name as the legacy name.
    pub fn new(name: &str) -> Self {
        Lay {
            name: name.to_owned(),
            rect: [0, 0, 1, 1],
            opacity: 255,
            clipping: 0,
            flags: 0,
            blocks: Vec::new(),
            channels: vec![(0, vec![7])],
        }
    }
    /// With an `lyid` block.
    pub fn id(self, id: u32) -> Self {
        self.block(b"lyid", &id.to_be_bytes())
    }
    /// With a tagged block.
    pub fn block(mut self, key: &[u8; 4], data: &[u8]) -> Self {
        self.blocks.push((*key, data.to_vec()));
        self
    }
    /// A group's own record (an open folder), with no channels' data.
    pub fn folder(name: &str) -> Self {
        let mut l = Lay::new(name).block(b"lsct", &be32(1));
        l.rect = [0, 0, 0, 0];
        l.channels = vec![(0, Vec::new())];
        l
    }
    /// A group's closing marker (the bounding section divider).
    pub fn end() -> Self {
        let mut l = Lay::new("</Layer group>").block(b"lsct", &be32(3));
        l.rect = [0, 0, 0, 0];
        l.channels = vec![(0, Vec::new())];
        l
    }
    /// Hidden.
    pub fn hidden(mut self) -> Self {
        self.flags |= 2;
        self
    }
    /// Its `[top, left, bottom, right]`.
    pub fn at(mut self, rect: [i32; 4]) -> Self {
        self.rect = rect;
        self
    }
    /// Its first channel's bytes.
    pub fn pixels(mut self, data: &[u8]) -> Self {
        self.channels[0].1 = data.to_vec();
        self
    }
}

/// `lay`'s stored record (`psb`: 8-byte channel lengths).
pub fn lay_record(psb: bool, lay: &Lay) -> Vec<u8> {
    let mut r = Vec::new();
    for v in lay.rect {
        r.extend(v.to_be_bytes());
    }
    r.extend(be16(lay.channels.len() as u16));
    for (id, data) in &lay.channels {
        r.extend(id.to_be_bytes());
        let len = 2 + data.len() as u64;
        if psb {
            r.extend(be64(len));
        } else {
            r.extend(be32(len as u32));
        }
    }
    r.extend_from_slice(b"8BIMnorm");
    r.extend([lay.opacity, lay.clipping, lay.flags, 0]);
    let mut blocks = Vec::new();
    for (key, data) in &lay.blocks {
        let mut d = data.clone();
        if d.len() % 2 == 1 {
            d.push(0);
        }
        blocks.extend_from_slice(b"8BIM");
        blocks.extend_from_slice(key);
        blocks.extend(be32(d.len() as u32));
        blocks.extend(d);
    }
    let name = lay.name.as_bytes();
    let e = extra(&name[..name.len().min(255)], &blocks);
    r.extend(be32(e.len() as u32));
    r.extend(e);
    r
}

/// A whole file holding `layers` (in file order: bottom first), then `global_blocks` after the
/// layer info, and the image resources `resources`.
pub fn psd_file(psb: bool, layers: &[Lay], global_blocks: &[u8], resources: &[u8]) -> Vec<u8> {
    let records: Vec<Vec<u8>> = layers.iter().map(|l| lay_record(psb, l)).collect();
    let mut chans = Vec::new();
    for l in layers {
        for (_, data) in &l.channels {
            let mut c = vec![0u8, 0];
            c.extend_from_slice(data);
            chans.push(c);
        }
    }
    let li = if layers.is_empty() {
        Vec::new()
    } else {
        layer_info(layers.len() as i16, &records, &chans)
    };
    hand_file(psb, 8, resources, &section(psb, &li, &[], global_blocks))
}

/// A `luni` block's data: the name as a count of UTF-16 units and the units.
pub fn luni(name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut d = be32(units.len() as u32).to_vec();
    for u in units {
        d.extend(u.to_be_bytes());
    }
    d
}
