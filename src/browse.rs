//! The Photos browser's file side: listing a folder in shooting order,
//! marking photos by renaming them, binning them, and quick previews.

use image::{DynamicImage, RgbImage, metadata::Orientation};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// What a marked photo's name starts with.
pub const MARK: &str = "A_";

/// Sidecar files other editors keep next to a photo (darktable / Lightroom,
/// RawTherapee, ART). They're renamed and binned along with it.
const SIDECARS: [&str; 3] = ["xmp", "pp3", "arp"];

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn is_marked(p: &Path) -> bool {
    file_name(p).starts_with(MARK)
}

/// The photo's name without its extension or its mark.
pub fn plain_stem(p: &Path) -> String {
    let s = stem(p);
    s.strip_prefix(MARK).map(str::to_string).unwrap_or(s)
}

/// An export from Still: its name ends with the export suffix.
pub fn is_edited_copy(p: &Path, suffix: &str) -> bool {
    !suffix.is_empty() && plain_stem(p).len() > suffix.len() && plain_stem(p).ends_with(suffix)
}

/// For an edited copy, the plain stem of the photo it was made from.
pub fn original_stem(p: &Path, suffix: &str) -> Option<String> {
    if !is_edited_copy(p, suffix) {
        return None;
    }
    let s = plain_stem(p);
    Some(s[..s.len() - suffix.len()].to_string())
}

/// The same path with the mark added, or taken off when it's there.
pub fn toggled(p: &Path) -> PathBuf {
    let name = file_name(p);
    let new = match name.strip_prefix(MARK) {
        Some(rest) => rest.to_string(),
        None => format!("{MARK}{name}"),
    };
    p.with_file_name(new)
}

/// Compares names the way people count: IMG_2 before IMG_10, case ignored.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Sort order: by name without the mark (so marking doesn't move a photo),
/// an edited copy right after its original, then by extension.
fn order(a: &Path, b: &Path) -> Ordering {
    natural_cmp(&plain_stem(a), &plain_stem(b))
        .then_with(|| natural_cmp(&a.extension().unwrap_or_default().to_string_lossy(), &b.extension().unwrap_or_default().to_string_lossy()))
        .then_with(|| file_name(a).cmp(&file_name(b)))
}

/// The photos in a folder (not its subfolders), in shooting order.
pub fn list(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && !file_name(p).starts_with('.') && crate::io::is_openable(p))
        .collect();
    files.sort_by(|a, b| order(a, b));
    Ok(files)
}

/// Sidecars belonging to a photo: `IMG_1.ARW.xmp` always, and `IMG_1.xmp`
/// when no other photo in the folder shares the stem (a RAW + JPEG pair).
fn sidecars(p: &Path) -> Vec<PathBuf> {
    let Some(dir) = p.parent() else { return Vec::new() };
    let (name, st) = (file_name(p), stem(p));
    let shared = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).any(|o| o != p && crate::io::is_openable(&o) && stem(&o) == st))
        .unwrap_or(true);
    let mut out = Vec::new();
    for ext in SIDECARS {
        for base in [Some(&name), (!shared).then_some(&st)].into_iter().flatten() {
            for e in [ext.to_string(), ext.to_uppercase()] {
                let s = dir.join(format!("{base}.{e}"));
                if s.is_file() && !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// Mark or unmark a photo by renaming it and its sidecars. Returns the new path.
pub fn toggle_mark(p: &Path) -> Result<PathBuf, String> {
    let to = toggled(p);
    if to.exists() {
        return Err(format!("{} already exists", file_name(&to)));
    }
    let side = sidecars(p);
    std::fs::rename(p, &to).map_err(|e| e.to_string())?;
    for s in side {
        let t = toggled(&s);
        if !t.exists() {
            let _ = std::fs::rename(&s, t);
        }
    }
    Ok(to)
}

/// Move a photo and its sidecars to the system Trash.
pub fn trash(p: &Path) -> Result<(), String> {
    let side = sidecars(p);
    trash::delete(p).map_err(|e| e.to_string())?;
    for s in side {
        let _ = trash::delete(s);
    }
    Ok(())
}

fn exif_orientation(src: &rawler::rawsource::RawSource, params: &rawler::decoders::RawDecodeParams) -> Option<Orientation> {
    rawler::get_decoder(src)
        .and_then(|d| d.raw_metadata(src, params))
        .ok()
        .and_then(|m| m.exif.orientation)
        .and_then(|o| Orientation::from_exif(o as u8))
}

/// The JPEG a camera stores inside a RAW, upright. Much quicker than developing it.
fn raw_preview(path: &Path) -> Option<DynamicImage> {
    let path = path.to_path_buf();
    std::panic::catch_unwind(move || {
        let src = rawler::rawsource::RawSource::new(&path).ok()?;
        let params = rawler::decoders::RawDecodeParams::default();
        let dec = rawler::get_decoder(&src).ok()?;
        let mut img = dec
            .preview_image(&src, &params)
            .ok()
            .flatten()
            .or_else(|| dec.full_image(&src, &params).ok().flatten())
            .or_else(|| dec.thumbnail_image(&src, &params).ok().flatten())?;
        if let Some(o) = exif_orientation(&src, &params) {
            img.apply_orientation(o);
        }
        Some(img)
    })
    .ok()
    .flatten()
}

fn fit(img: &DynamicImage, edge: u32) -> RgbImage {
    let img = if img.width().max(img.height()) > edge { img.thumbnail(edge, edge) } else { img.clone() };
    // transparent pictures sit on black, like the canvas
    img.to_rgb8()
}

/// Load a photo for the browser: the big picture (when `view` gives its long
/// edge) and a thumbnail. RAWs use their embedded preview when they have one.
pub fn load(path: &Path, view: Option<u32>, thumb: u32) -> Result<(Option<RgbImage>, RgbImage), String> {
    let img = match crate::io::is_raw(path).then(|| raw_preview(path)).flatten() {
        Some(i) => i,
        None => crate::io::decode(path)?,
    };
    Ok((view.map(|e| fit(&img, e)), fit(&img, thumb)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_and_marks_keep_place() {
        let mut v: Vec<PathBuf> = ["IMG_10.png", "A_IMG_2.png", "IMG_1.png", "IMG_1-edited.jpg", "img_3.JPG"].iter().map(PathBuf::from).collect();
        v.sort_by(|a, b| order(a, b));
        let names: Vec<String> = v.iter().map(|p| file_name(p)).collect();
        assert_eq!(names, ["IMG_1.png", "IMG_1-edited.jpg", "A_IMG_2.png", "img_3.JPG", "IMG_10.png"]);
    }

    #[test]
    fn marks_and_edited_copies() {
        let p = Path::new("/x/DSC00747.ARW");
        assert_eq!(toggled(p), Path::new("/x/A_DSC00747.ARW"));
        assert_eq!(toggled(&toggled(p)), p);
        assert!(is_marked(&toggled(p)));
        let e = Path::new("/x/A_DSC00747-edited.jpg");
        assert!(is_edited_copy(e, "-edited"));
        assert_eq!(original_stem(e, "-edited").as_deref(), Some("DSC00747"));
        assert_eq!(original_stem(p, "-edited"), None);
        assert_eq!(plain_stem(e), "DSC00747-edited");
    }
}

