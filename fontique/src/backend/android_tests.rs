// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::*;
use crate::source::SourceKind;
use std::format;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const API30_XML: &str = include_str!("test_data/android_api30_fallbacks.xml");
const CJK_FILE: &str = "NotoSansCJK-Regular.ttc";

/// A real four-face collection, built from the repository's small font fixtures.
/// Each face has a distinct family, making the XML collection index observable.
struct Fonts {
    dir: PathBuf,
}

impl Fonts {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "fontique-android-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let faces: &[&[u8]] = &[
            include_bytes!("../../../parley_dev/assets/fonts/roboto_fonts/Roboto-Regular.ttf"),
            include_bytes!(
                "../../../parley_dev/assets/fonts/arimo_fonts/Arimo-VariableFont_wght.ttf"
            ),
            include_bytes!(
                "../../../parley_dev/assets/fonts/noto_fonts/NotoSansCJKsc-Regular-subset.otf"
            ),
            include_bytes!(
                "../../../parley_dev/assets/fonts/noto_fonts/NotoKufiArabic-Regular.otf"
            ),
        ];
        std::fs::write(dir.join(CJK_FILE), collection(faces)).unwrap();
        std::fs::write(
            dir.join("NotoColorEmoji.ttf"),
            include_bytes!(
                "../../../parley_dev/assets/fonts/noto_color_emoji/NotoColorEmoji-CBTF-Subset.ttf"
            ),
        )
        .unwrap();
        Self { dir }
    }

    fn scan(&self) -> scan::ScannedCollection {
        scan::ScannedCollection::from_paths([&self.dir], 0)
    }

    fn system(&self, xml: &str) -> SystemFonts {
        SystemFonts::from_scanned_collection(self.scan(), &self.dir, Some(xml))
    }

    fn postscript(&self, index: u32) -> String {
        let scanned = self.scan();
        let family = scanned
            .families
            .values()
            .find(|family| {
                family.fonts().iter().any(|font| {
                    matches!(&font.source().kind, SourceKind::Path(path) if path.file_name().unwrap() == CJK_FILE)
                        && font.index() == index
                })
            })
            .unwrap();
        scanned
            .postscript_names
            .iter()
            .find(|(_, id)| **id == family.id())
            .unwrap()
            .0
            .clone()
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn collection(faces: &[&[u8]]) -> Vec<u8> {
    let count = u32::try_from(faces.len()).unwrap();
    let mut data = b"ttcf\0\x01\0\0".to_vec();
    data.extend_from_slice(&count.to_be_bytes());
    data.resize(12 + faces.len() * 4, 0);
    for (index, face) in faces.iter().enumerate() {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let offset = u32::try_from(data.len()).unwrap();
        data[12 + index * 4..16 + index * 4].copy_from_slice(&offset.to_be_bytes());
        let start = data.len();
        data.extend_from_slice(face);
        let tables = u16::from_be_bytes(face[4..6].try_into().unwrap());
        for table in 0..usize::from(tables) {
            let field = 12 + table * 16 + 8;
            let original = u32::from_be_bytes(face[field..field + 4].try_into().unwrap());
            data[start + field..start + field + 4]
                .copy_from_slice(&(original + offset).to_be_bytes());
        }
    }
    data
}

fn assert_face(system: &SystemFonts, key: impl Into<FallbackKey>, file: &str, index: u32) {
    let id = system.fallback(key).expect("fallback family");
    let family = system.family(id).expect("scanned family");
    let font = family.default_font().expect("scanned font");
    assert_eq!(font.index(), index, "wrong collection face for {file}");
    let SourceKind::Path(path) = &font.source().kind else {
        panic!("expected a scanned file source");
    };
    assert_eq!(path.file_name().unwrap(), file);
}

#[test]
fn scanner_retains_distinct_ttc_faces_and_sources() {
    let fonts = Fonts::new();
    let scanned = fonts.scan();
    for index in 0..4 {
        assert!(scanned.families.values().any(|family| {
            family.fonts().iter().any(|font| {
                matches!(&font.source().kind, SourceKind::Path(path) if **path == fonts.dir.join(CJK_FILE))
                    && font.index() == index
            })
        }));
        assert!(!fonts.postscript(index).is_empty());
    }
}

#[test]
fn api30_filename_fallbacks_respect_ttc_indices_and_han_locales() {
    let system = Fonts::new().system(API30_XML);
    for (locale, index) in [
        ("ja", 0),
        ("ja-JP", 0),
        ("ko", 1),
        ("ko-KR", 1),
        ("zh-Hans", 2),
        ("zh-CN", 2),
        ("zh-SG", 2),
        ("zh-Hant", 3),
        ("zh-TW", 3),
        ("zh-HK", 3),
        ("zh-MO", 3),
    ] {
        assert_face(
            &system,
            (Script::from_bytes(*b"Hani"), locale),
            CJK_FILE,
            index,
        );
    }
    assert_face(
        &system,
        (Script::from_bytes(*b"Bopo"), "zh-Bopo"),
        CJK_FILE,
        3,
    );
}

#[test]
fn default_han_follows_first_xml_script_fallback() {
    let system = Fonts::new().system(API30_XML);
    assert_face(&system, Script::from_bytes(*b"Hani"), CJK_FILE, 2);
    assert_face(
        &system,
        (Script::from_bytes(*b"Hani"), "en-US"),
        CJK_FILE,
        2,
    );
}

#[test]
fn standalone_filename_defaults_to_index_zero() {
    let system = Fonts::new().system(API30_XML);
    assert_face(
        &system,
        Script::from_bytes(*b"Zsye"),
        "NotoColorEmoji.ttf",
        0,
    );
}

#[test]
fn invalid_or_missing_filename_faces_do_not_create_fallbacks() {
    let fonts = Fonts::new();
    for (index, file) in [
        ("nope", CJK_FILE),
        ("-1", CJK_FILE),
        ("4", CJK_FILE),
        ("4294967296", CJK_FILE),
        ("0", "missing.ttf"),
    ] {
        let system = fonts.system(&format!(
            "<familyset version='23'><family lang='und-Arab'><font index='{index}'>{file}</font></family></familyset>"
        ));
        assert!(
            system.script_fallback.is_empty(),
            "accepted {file}, index {index}"
        );
    }
}

#[test]
fn postscript_name_has_precedence_over_filename_and_index() {
    let fonts = Fonts::new();
    let ps = fonts.postscript(1);
    let system = fonts.system(&format!(
        "<familyset version='23'><family lang='und-Arab'><font postScriptName='{ps}' index='2'>{CJK_FILE}</font></family></familyset>"
    ));
    assert_face(&system, Script::from_bytes(*b"Arab"), CJK_FILE, 1);
}

#[test]
fn unknown_postscript_name_uses_filename_and_trims_text() {
    let fonts = Fonts::new();
    let system = fonts.system(&format!(
        "<familyset version='23'><family lang='und-Arab'><font postScriptName='MissingPS' index='2'>\n  {CJK_FILE} \n</font></family></familyset>"
    ));
    assert_face(&system, Script::from_bytes(*b"Arab"), CJK_FILE, 2);
}

#[test]
fn family_specific_fallbacks_are_excluded_from_general_resolution() {
    let fonts = Fonts::new();
    let ps = fonts.postscript(1);
    let system = fonts.system(&format!(
        "<familyset version='23'><family lang='und-Arab'><font fallbackFor='serif' postScriptName='{ps}' index='1'>{CJK_FILE}</font><font index='2'>{CJK_FILE}</font></family></familyset>"
    ));
    assert_face(&system, Script::from_bytes(*b"Arab"), CJK_FILE, 2);
    let only_specific = fonts.system(&format!(
        "<familyset version='23'><family lang='und-Arab'><font fallbackFor='serif' postScriptName='{ps}'>{CJK_FILE}</font></family></familyset>"
    ));
    assert!(only_specific.script_fallback.is_empty());
}

#[test]
fn general_fonts_keep_xml_order_and_ignore_non_font_elements() {
    let fonts = Fonts::new();
    let ps = fonts.postscript(1);
    let system = fonts.system(&format!(
        "<familyset version='23'><family lang='und-Arab'><axis postScriptName='{ps}'/><font index='2'>{CJK_FILE}</font><font postScriptName='{ps}'>{CJK_FILE}</font></family></familyset>"
    ));
    assert_face(&system, Script::from_bytes(*b"Arab"), CJK_FILE, 2);
}

#[test]
fn postscript_han_fallbacks_canonicalize_locales_and_preserve_regional_fonts() {
    let fonts = Fonts::new();
    let traditional = fonts.postscript(3);
    let simplified = fonts.postscript(2);
    let hong_kong = fonts.postscript(1);
    let japanese = fonts.postscript(0);
    let system = fonts.system(&format!(
        "<familyset version='23'>
         <family lang='zh-Hant'><font postScriptName='{traditional}'/></family>
         <family lang='zh-Hans'><font postScriptName='{simplified}'/></family>
         <family lang='zh-HK'><font postScriptName='{hong_kong}'/></family>
         <family lang='ja'><font postScriptName='{japanese}'/></family>
         </familyset>"
    ));
    for (locale, index) in [
        ("zh-CN", 2),
        ("zh-Hans", 2),
        ("zh-SG", 2),
        ("zh-TW", 3),
        ("zh-Hant", 3),
        ("zh-MO", 3),
        ("zh-HK", 1),
        ("ja-JP", 0),
    ] {
        assert_face(
            &system,
            (Script::from_bytes(*b"Hani"), locale),
            CJK_FILE,
            index,
        );
    }
    assert_face(&system, Script::from_bytes(*b"Hani"), CJK_FILE, 3);
}
