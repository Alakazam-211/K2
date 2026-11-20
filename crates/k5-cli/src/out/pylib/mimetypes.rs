//! Python 3.12's `mimetypes.guess_type(name)`, as the shell's Python
//! calls it with the module's own database: the built-in map, then each
//! of [`KNOWNFILES`] that is a file, read in order, a later entry
//! replacing an earlier one.
//!
//! Copied from CPython's `Lib/mimetypes.py` on the 3.12 branch, from
//! v3.12.5 on (checked against v3.12.16): `guess_type` reads a scheme with
//! `urllib.parse.urlparse` (v3.12.3 on), and the built-in map has
//! `.md`, `.markdown` (text/markdown) and `.rst` (text/x-rst) (v3.12.5
//! on). Before that, v3.12.0 to v3.12.2 read a scheme with `_splittype`
//! and v3.12.0 to v3.12.4 have no `.md`, `.markdown` or `.rst`; a system
//! `mime.types` usually names those anyway. Only the strict map is here:
//! `guess_type` is called with `strict=True`. The Windows registry is not
//! read (the shell CLI runs on Unix).

use crate::out::pylib::json::{read_text_file, Failed};
use std::collections::HashMap;

/// `mimetypes.knownfiles` on a Unix, in the order `init()` reads them.
pub const KNOWNFILES: &[&str] = &[
    "/etc/mime.types",
    "/etc/httpd/mime.types",
    "/etc/httpd/conf/mime.types",
    "/etc/apache/mime.types",
    "/etc/apache2/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/lib/netscape/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/etc/mime.types",
];

/// `_suffix_map_default`.
const SUFFIX_MAP: &[(&str, &str)] = &[
    (".svgz", ".svg.gz"),
    (".tgz", ".tar.gz"),
    (".taz", ".tar.gz"),
    (".tz", ".tar.gz"),
    (".tbz2", ".tar.bz2"),
    (".txz", ".tar.xz"),
];

/// `_encodings_map_default` (its keys are matched case and all).
const ENCODINGS_MAP: &[(&str, &str)] = &[
    (".gz", "gzip"),
    (".Z", "compress"),
    (".bz2", "bzip2"),
    (".xz", "xz"),
    (".br", "br"),
];

/// `_types_map_default`, in the source's order.
const TYPES_MAP: &[(&str, &str)] = &[
    (".js", "text/javascript"),
    (".mjs", "text/javascript"),
    (".json", "application/json"),
    (".webmanifest", "application/manifest+json"),
    (".doc", "application/msword"),
    (".dot", "application/msword"),
    (".wiz", "application/msword"),
    (".nq", "application/n-quads"),
    (".nt", "application/n-triples"),
    (".bin", "application/octet-stream"),
    (".a", "application/octet-stream"),
    (".dll", "application/octet-stream"),
    (".exe", "application/octet-stream"),
    (".o", "application/octet-stream"),
    (".obj", "application/octet-stream"),
    (".so", "application/octet-stream"),
    (".oda", "application/oda"),
    (".pdf", "application/pdf"),
    (".p7c", "application/pkcs7-mime"),
    (".ps", "application/postscript"),
    (".ai", "application/postscript"),
    (".eps", "application/postscript"),
    (".trig", "application/trig"),
    (".m3u", "application/vnd.apple.mpegurl"),
    (".m3u8", "application/vnd.apple.mpegurl"),
    (".xls", "application/vnd.ms-excel"),
    (".xlb", "application/vnd.ms-excel"),
    (".ppt", "application/vnd.ms-powerpoint"),
    (".pot", "application/vnd.ms-powerpoint"),
    (".ppa", "application/vnd.ms-powerpoint"),
    (".pps", "application/vnd.ms-powerpoint"),
    (".pwz", "application/vnd.ms-powerpoint"),
    (".wasm", "application/wasm"),
    (".bcpio", "application/x-bcpio"),
    (".cpio", "application/x-cpio"),
    (".csh", "application/x-csh"),
    (".dvi", "application/x-dvi"),
    (".gtar", "application/x-gtar"),
    (".hdf", "application/x-hdf"),
    (".h5", "application/x-hdf5"),
    (".latex", "application/x-latex"),
    (".mif", "application/x-mif"),
    (".cdf", "application/x-netcdf"),
    (".nc", "application/x-netcdf"),
    (".p12", "application/x-pkcs12"),
    (".pfx", "application/x-pkcs12"),
    (".ram", "application/x-pn-realaudio"),
    (".pyc", "application/x-python-code"),
    (".pyo", "application/x-python-code"),
    (".sh", "application/x-sh"),
    (".shar", "application/x-shar"),
    (".swf", "application/x-shockwave-flash"),
    (".sv4cpio", "application/x-sv4cpio"),
    (".sv4crc", "application/x-sv4crc"),
    (".tar", "application/x-tar"),
    (".tcl", "application/x-tcl"),
    (".tex", "application/x-tex"),
    (".texi", "application/x-texinfo"),
    (".texinfo", "application/x-texinfo"),
    (".roff", "application/x-troff"),
    (".t", "application/x-troff"),
    (".tr", "application/x-troff"),
    (".man", "application/x-troff-man"),
    (".me", "application/x-troff-me"),
    (".ms", "application/x-troff-ms"),
    (".ustar", "application/x-ustar"),
    (".src", "application/x-wais-source"),
    (".xsl", "application/xml"),
    (".rdf", "application/xml"),
    (".wsdl", "application/xml"),
    (".xpdl", "application/xml"),
    (".zip", "application/zip"),
    (".3gp", "audio/3gpp"),
    (".3gpp", "audio/3gpp"),
    (".3g2", "audio/3gpp2"),
    (".3gpp2", "audio/3gpp2"),
    (".aac", "audio/aac"),
    (".adts", "audio/aac"),
    (".loas", "audio/aac"),
    (".ass", "audio/aac"),
    (".au", "audio/basic"),
    (".snd", "audio/basic"),
    (".mp3", "audio/mpeg"),
    (".mp2", "audio/mpeg"),
    (".opus", "audio/opus"),
    (".aif", "audio/x-aiff"),
    (".aifc", "audio/x-aiff"),
    (".aiff", "audio/x-aiff"),
    (".ra", "audio/x-pn-realaudio"),
    (".wav", "audio/x-wav"),
    (".avif", "image/avif"),
    (".bmp", "image/bmp"),
    (".gif", "image/gif"),
    (".ief", "image/ief"),
    (".jpg", "image/jpeg"),
    (".jpe", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".heic", "image/heic"),
    (".heif", "image/heif"),
    (".png", "image/png"),
    (".svg", "image/svg+xml"),
    (".tiff", "image/tiff"),
    (".tif", "image/tiff"),
    (".ico", "image/vnd.microsoft.icon"),
    (".ras", "image/x-cmu-raster"),
    (".pnm", "image/x-portable-anymap"),
    (".pbm", "image/x-portable-bitmap"),
    (".pgm", "image/x-portable-graymap"),
    (".ppm", "image/x-portable-pixmap"),
    (".rgb", "image/x-rgb"),
    (".xbm", "image/x-xbitmap"),
    (".xpm", "image/x-xpixmap"),
    (".xwd", "image/x-xwindowdump"),
    (".eml", "message/rfc822"),
    (".mht", "message/rfc822"),
    (".mhtml", "message/rfc822"),
    (".nws", "message/rfc822"),
    (".css", "text/css"),
    (".csv", "text/csv"),
    (".html", "text/html"),
    (".htm", "text/html"),
    (".md", "text/markdown"),
    (".markdown", "text/markdown"),
    (".n3", "text/n3"),
    (".txt", "text/plain"),
    (".bat", "text/plain"),
    (".c", "text/plain"),
    (".h", "text/plain"),
    (".ksh", "text/plain"),
    (".pl", "text/plain"),
    (".srt", "text/plain"),
    (".rtx", "text/richtext"),
    (".tsv", "text/tab-separated-values"),
    (".vtt", "text/vtt"),
    (".py", "text/x-python"),
    (".rst", "text/x-rst"),
    (".etx", "text/x-setext"),
    (".sgm", "text/x-sgml"),
    (".sgml", "text/x-sgml"),
    (".vcf", "text/x-vcard"),
    (".xml", "text/xml"),
    (".mp4", "video/mp4"),
    (".mpeg", "video/mpeg"),
    (".m1v", "video/mpeg"),
    (".mpa", "video/mpeg"),
    (".mpe", "video/mpeg"),
    (".mpg", "video/mpeg"),
    (".mov", "video/quicktime"),
    (".qt", "video/quicktime"),
    (".webm", "video/webm"),
    (".avi", "video/x-msvideo"),
    (".movie", "video/x-sgi-movie"),
];

/// `urllib.parse.uses_params`: the schemes whose path loses `;…`.
const USES_PARAMS: &[&str] = &[
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu",
    "sip", "sips", "mms", "sftp", "tel",
];

/// A `MimeTypes` database: its strict extension map.
#[derive(Debug, Clone)]
pub struct MimeTypes {
    types: HashMap<String, String>,
}

impl Default for MimeTypes {
    fn default() -> Self {
        MimeTypes::builtin()
    }
}

impl MimeTypes {
    /// `MimeTypes()`: the built-in map only.
    pub fn builtin() -> MimeTypes {
        MimeTypes {
            types: TYPES_MAP
                .iter()
                .map(|(e, t)| (e.to_string(), t.to_string()))
                .collect(),
        }
    }

    /// `add_type(type, ext)`: an empty type is skipped (3.12.10 on; a
    /// `mime.types` line never gives one).
    pub fn add_type(&mut self, mime: &str, ext: &str) {
        if !mime.is_empty() {
            self.types.insert(ext.to_string(), mime.to_string());
        }
    }

    /// `readfp`: each line's words (`str.split()`), up to a word that
    /// starts with `#`; the first is the type, each other one an
    /// extension without its dot.
    pub fn read_text(&mut self, text: &str) {
        for line in text.split_inclusive('\n') {
            let mut words: Vec<&str> = Vec::new();
            for w in line
                .split(crate::families::py_space)
                .filter(|w| !w.is_empty())
            {
                if w.starts_with('#') {
                    break;
                }
                words.push(w);
            }
            let Some((mime, suffixes)) = words.split_first() else {
                continue;
            };
            for s in suffixes {
                self.add_type(mime, &format!(".{s}"));
            }
        }
    }

    /// `read(filename)`: `open(filename, encoding="utf-8")` (universal
    /// newlines), then [`MimeTypes::read_text`]; or what Python raises.
    pub fn read(&mut self, path: &str) -> Result<(), Failed> {
        let text = read_text_file(path)?;
        self.read_text(&text);
        Ok(())
    }

    /// `guess_type(url)[0]` (`strict=True`).
    pub fn guess_type(&self, url: &str) -> Option<String> {
        self.guess(url).0
    }

    /// `guess_type(url)`: the type and the encoding.
    pub fn guess(&self, url: &str) -> (Option<String>, Option<&'static str>) {
        let (scheme, url) = match urlparse(url) {
            Some((scheme, path)) if scheme.chars().count() > 1 => (Some(scheme), path),
            // posixpath.splitdrive: no drive.
            _ => (None, url.to_string()),
        };
        if scheme.as_deref() == Some("data") {
            let Some(comma) = url.find(',') else {
                return (None, None);
            };
            let head = &url[..comma];
            let mime = match head.find(';') {
                Some(semi) => &head[..semi],
                None => head,
            };
            let mime = if mime.contains('=') || !mime.contains('/') {
                "text/plain"
            } else {
                mime
            };
            return (Some(mime.to_string()), None);
        }
        let (mut base, mut ext) = splitext(&url);
        while let Some((_, to)) = SUFFIX_MAP
            .iter()
            .find(|(from, _)| *from == ext.to_lowercase())
        {
            let (b, e) = splitext(&format!("{base}{to}"));
            base = b;
            ext = e;
        }
        let encoding = match ENCODINGS_MAP.iter().find(|(e, _)| *e == ext) {
            Some((_, enc)) => {
                let (_, e) = splitext(&base);
                ext = e;
                Some(*enc)
            }
            None => None,
        };
        (self.types.get(&ext.to_lowercase()).cloned(), encoding)
    }
}

/// `mimetypes.init()` with `files` as `knownfiles`: the built-in map,
/// then each of `files` that `os.path.isfile` takes, in order. What
/// Python raises reading one (a file that is no UTF-8, one it may not
/// open) comes back as the error.
pub fn init(files: &[&str]) -> Result<MimeTypes, Failed> {
    let mut db = MimeTypes::builtin();
    for f in files {
        if std::path::Path::new(f).is_file() {
            db.read(f)?;
        }
    }
    Ok(db)
}

/// The module's own database: [`init`] with [`KNOWNFILES`].
pub fn system() -> Result<MimeTypes, Failed> {
    init(KNOWNFILES)
}

/// `posixpath.splitext(p)`: the root and the extension (with its dot;
/// leading dots of the name are no extension).
pub fn splitext(p: &str) -> (String, String) {
    let sep = p.rfind('/').map_or(0, |i| i + 1);
    if let Some(dot) = p.rfind('.') {
        if dot >= sep && p[sep..dot].chars().any(|c| c != '.') {
            return (p[..dot].to_string(), p[dot..].to_string());
        }
    }
    (p.to_string(), String::new())
}

/// `_WHATWG_C0_CONTROL_OR_SPACE`.
fn c0_or_space(c: char) -> bool {
    c <= ' '
}

/// `urllib.parse.urlparse(url)`'s scheme and path, when it finds a
/// scheme; `None` when it finds none (the caller then keeps the text as
/// it was). A file name has no `/`, so no network location.
fn urlparse(url: &str) -> Option<(String, String)> {
    // urlsplit: leading C0 controls and spaces go, then every tab, CR
    // and LF.
    let url: String = url
        .trim_start_matches(c0_or_space)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let i = url.find(':')?;
    let first = url.chars().next()?;
    if i == 0 || !first.is_ascii_alphabetic() {
        return None;
    }
    let scheme_ok = url[..i]
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok {
        return None;
    }
    let scheme = url[..i].to_ascii_lowercase();
    let mut rest = url[i + 1..].to_string();
    if let Some(after) = rest.strip_prefix("//") {
        // _splitnetloc: up to the first of /?# (only a path is kept).
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        rest = after[end..].to_string();
    }
    if let Some(hash) = rest.find('#') {
        rest.truncate(hash);
    }
    if let Some(q) = rest.find('?') {
        rest.truncate(q);
    }
    if USES_PARAMS.contains(&scheme.as_str()) && rest.contains(';') {
        // _splitparams.
        let cut = match rest.rfind('/') {
            Some(slash) => rest[slash..].find(';').map(|j| slash + j),
            None => rest.find(';'),
        };
        if let Some(cut) = cut {
            rest.truncate(cut);
        }
    }
    Some((scheme, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guess(name: &str) -> Option<String> {
        MimeTypes::builtin().guess_type(name)
    }

    #[test]
    fn the_built_in_map_as_python_3_12_reads_it() {
        let want = [
            ("logo.png", Some("image/png")),
            ("LOGO.PNG", Some("image/png")),
            ("a.svg", Some("image/svg+xml")),
            ("a.svgz", Some("image/svg+xml")),
            ("a.tar.gz", Some("application/x-tar")),
            ("a.tgz", Some("application/x-tar")),
            ("a.TGZ", Some("application/x-tar")),
            ("a.gz", None),
            ("a.tar.GZ", None),
            ("a.json", Some("application/json")),
            ("a.md", Some("text/markdown")),
            ("a.webp", None),
            ("a.woff2", None),
            ("noext", None),
            (".png", None),
            ("..png", None),
            ("a.", None),
            ("a.b.c.txt", Some("text/plain")),
            ("\u{0130}.PNG", Some("image/png")),
        ];
        for (name, mime) in want {
            assert_eq!(guess(name).as_deref(), mime, "{name:?}");
        }
    }

    #[test]
    fn a_name_with_a_colon_is_read_as_python_s_urlparse_reads_it() {
        let want = [
            // One letter is no scheme (a drive): the whole name.
            ("a:b.png", Some("image/png")),
            // A scheme: its path.
            ("ab:c.png", Some("image/png")),
            ("ab:c.png?x", Some("image/png")),
            ("ab:c.png#x", Some("image/png")),
            ("1b:c.txt?x", None),
            // http takes ;params off the path.
            ("http:x.png;y.txt", Some("image/png")),
            ("zz:x.png;y.txt", Some("text/plain")),
            // data: the type in the name, else text/plain.
            ("data:image/gif,xyz", Some("image/gif")),
            ("data:image/gif;base64,xyz", Some("image/gif")),
            ("data:a=b,xyz", Some("text/plain")),
            ("data:plain,xyz", Some("text/plain")),
            ("data:image/gif", None),
            ("DATA:image/gif,x", Some("image/gif")),
            (" ab:c.png", Some("image/png")),
        ];
        for (name, mime) in want {
            assert_eq!(guess(name).as_deref(), mime, "{name:?}");
        }
    }

    #[test]
    fn splitext_as_posixpath() {
        let want = [
            ("a.b", ("a", ".b")),
            (".a", (".a", "")),
            ("..a", ("..a", "")),
            ("...a.b", ("...a", ".b")),
            ("a", ("a", "")),
            ("d.x/a", ("d.x/a", "")),
            ("d/.a", ("d/.a", "")),
            ("a.", ("a", ".")),
        ];
        for (p, (root, ext)) in want {
            assert_eq!(splitext(p), (root.to_string(), ext.to_string()), "{p:?}");
        }
    }

    #[test]
    fn the_encoding_comes_back_too() {
        let db = MimeTypes::builtin();
        assert_eq!(
            db.guess("a.tar.gz"),
            (Some("application/x-tar".into()), Some("gzip"))
        );
        assert_eq!(db.guess("a.Z"), (None, Some("compress")));
        assert_eq!(db.guess("a.z"), (None, None));
    }

    /// The reader takes its files in order, a later entry replacing an
    /// earlier one and the built-in map; a missing file or a folder is
    /// skipped; comments and blank lines are nothing.
    #[test]
    fn known_files_are_read_in_order_and_a_later_entry_wins() {
        let dir = std::env::temp_dir().join(format!("k5-mimetypes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a-folder")).unwrap();
        let first = dir.join("first.types");
        let second = dir.join("second.types");
        std::fs::write(
            &first,
            "# a comment line\n\napplication/x-acme png acme\r\ntext/x-one one #two\nimage/x-old woff2\n",
        )
        .unwrap();
        std::fs::write(&second, "font/woff2\twoff2\u{2028}x-extra\n  \nimage/x-cap PNG\n").unwrap();
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let (first, second, missing, folder) = (
            s(&first),
            s(&second),
            s(&dir.join("missing.types")),
            s(&dir.join("a-folder")),
        );
        let db = init(&[&missing, &first, &folder, &second]).unwrap();
        // The custom entry overrides the built-in map.
        assert_eq!(db.guess_type("a.png").as_deref(), Some("application/x-acme"));
        assert_eq!(db.guess_type("a.acme").as_deref(), Some("application/x-acme"));
        assert_eq!(db.guess_type("a.one").as_deref(), Some("text/x-one"));
        // `#two` and what follows it is a comment.
        assert_eq!(db.guess_type("a.two"), None);
        // The second file replaces the first's entry.
        assert_eq!(db.guess_type("a.woff2").as_deref(), Some("font/woff2"));
        // U+2028 is whitespace to str.split(), not a line break to readline.
        assert_eq!(db.guess_type("a.x-extra").as_deref(), Some("font/woff2"));
        // An upper-case extension in a file is never matched (the name's
        // extension is looked up in lower case).
        assert_eq!(db.guess_type("a.PNG").as_deref(), Some("application/x-acme"));
        // The built-in map is still there for the rest.
        assert_eq!(db.guess_type("a.gif").as_deref(), Some("image/gif"));

        // Read the other way round, the first file's entries win.
        let db = init(&[&second, &first]).unwrap();
        assert_eq!(db.guess_type("a.woff2").as_deref(), Some("image/x-old"));

        // A file that is not UTF-8 is Python's UnicodeDecodeError.
        let bad = dir.join("bad.types");
        std::fs::write(&bad, b"image/x \xff\n").unwrap();
        let err = init(&[&s(&bad)]).unwrap_err();
        assert_eq!(err.class, "UnicodeDecodeError");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_known_files_are_python_s() {
        assert_eq!(KNOWNFILES.len(), 9);
        assert_eq!(KNOWNFILES[0], "/etc/mime.types");
        assert_eq!(KNOWNFILES[4], "/etc/apache2/mime.types");
        assert_eq!(TYPES_MAP.len(), 152);
    }
}
