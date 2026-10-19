// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::{
    boxed::Box,
    str::FromStr,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use std::path::{Path, PathBuf};

use hashbrown::HashMap;
use roxmltree::Document;

use crate::source::SourceKind;

use super::{
    FallbackKey, FamilyId, FamilyInfo, FamilyNameMap, GenericFamily, GenericFamilyMap, Language,
    Script, scan,
};

// TODO: Use actual generic families here, where available, when fonts.xml is properly parsed.
//       system-ui should map to `variant="compact"` in some scripts during fallback resolution.
const DEFAULT_GENERIC_FAMILIES: &[(GenericFamily, &[&str])] = &[
    (
        GenericFamily::SansSerif,
        &["Roboto Flex", "Roboto", "Noto Sans"],
    ),
    (GenericFamily::Serif, &["Noto Serif"]),
    (GenericFamily::Monospace, &["monospace"]),
    (GenericFamily::Cursive, &["Dancing Script"]),
    (GenericFamily::Fantasy, &["Noto Serif"]),
    (
        GenericFamily::SystemUi,
        &["Roboto Flex", "Roboto", "Noto Sans"],
    ),
    (GenericFamily::Emoji, &["Noto Color Emoji"]),
    (GenericFamily::Math, &["Noto Sans Math", "Noto Sans"]),
];

pub(crate) struct SystemFonts {
    pub(crate) name_map: Arc<FamilyNameMap>,
    pub(crate) generic_families: Arc<GenericFamilyMap>,
    family_map: HashMap<FamilyId, FamilyInfo>,
    locale_fallback: Box<[(Language, FamilyId)]>,
    script_fallback: Box<[(Script, FamilyId)]>,
}

impl SystemFonts {
    pub(crate) fn new() -> Self {
        let android_root: String = std::env::var("ANDROID_ROOT").unwrap_or("/system".to_string());
        let fonts_dir = Path::new(&android_root).join("fonts");
        let scanned = scan::ScannedCollection::from_paths([&fonts_dir], 8);
        let xml = std::fs::read_to_string(Path::new(&android_root).join("etc/fonts.xml")).ok();
        Self::from_scanned_collection(scanned, &fonts_dir, xml.as_deref())
    }

    fn from_scanned_collection(
        scanned: scan::ScannedCollection,
        fonts_dir: &Path,
        xml: Option<&str>,
    ) -> Self {
        let scan::ScannedCollection {
            family_names: mut name_map,
            families: family_map,
            postscript_names,
            ..
        } = scanned;
        // XML on older Android versions identifies faces by filename and TTC
        // index instead of postScriptName. Resolve these against the scanner's
        // actual sources rather than guessing a family name from the filename.
        let mut file_families: HashMap<(PathBuf, u32), FamilyId> = HashMap::default();
        for (id, family) in &family_map {
            for font in family.fonts() {
                if let SourceKind::Path(path) = font.source().kind() {
                    file_families.insert((path.to_path_buf(), font.index()), *id);
                }
            }
        }
        let mut generic_families = GenericFamilyMap::default();
        for (family, names) in DEFAULT_GENERIC_FAMILIES {
            generic_families.set(
                *family,
                names
                    .iter()
                    .filter_map(|name| name_map.get(name))
                    .map(|name| name.id()),
            );
        }

        let mut locale_fallback = vec![];
        let mut script_fallback = vec![];

        // Try to get generic info from fonts.xml
        if let Some(s) = xml
            && let Ok(doc) = Document::parse(s)
        {
            let root = doc.root_element();
            if root.tag_name().name() == "familyset"
                || root
                    .attribute("version")
                    .is_some_and(|v| usize::from_str(v).is_ok_and(|x| x >= 21))
            {
                for child in root.children() {
                    match child.tag_name().name() {
                        "alias" => {
                            if let Some((name, to)) =
                                child.attribute("name").zip(child.attribute("to"))
                            {
                                if child.attribute("weight").is_some() {
                                    // weight aliases are an Android quirk and are not in­
                                    // teresting for use cases other than Android legacy.
                                    continue;
                                }
                                let to_n = name_map.get_or_insert(to);
                                name_map.add_alias(to_n.id(), name);
                            }
                        }
                        "family" => {
                            if let Some(name) = child.attribute("name") {
                                let f = name_map.get_or_insert(name);
                                let _id = f.id();
                                for _child in child.children() {
                                    // TODO: map using postScriptName when available other­
                                    //       wise use the file name, and perhaps if necess­
                                    //       ary (e.g. if it's a collection), do something
                                    //       smarter, or something dumb that meets expecta­
                                    //       tions on Android.
                                }
                            } else if let Some(langs) = child
                                .attribute("lang")
                                .map(|s| s.split(',').map(str::trim).collect::<Vec<&str>>())
                            {
                                // Only general fallback fonts participate here. Keep XML
                                // order, preferring each font's PostScript name when known.
                                let family = child
                                    .children()
                                    .filter(|font| {
                                        font.has_tag_name("font")
                                            && font.attribute("fallbackFor").is_none()
                                    })
                                    .find_map(|font| {
                                        font.attribute("postScriptName")
                                            .and_then(|name| postscript_names.get(name))
                                            .copied()
                                            .or_else(|| {
                                                let index = font
                                                    .attribute("index")
                                                    .map(str::parse::<u32>)
                                                    .transpose()
                                                    .ok()?
                                                    .unwrap_or(0);
                                                let filename = font.text()?.trim();
                                                file_families
                                                    .get(&(fonts_dir.join(filename), index))
                                                    .copied()
                                            })
                                    });
                                if let Some(family) = family {
                                    for lang in langs {
                                        if let Some(scr) = lang.strip_prefix("und-") {
                                            // Undefined lang for script-only fallbacks
                                            script_fallback.push((
                                                scr.parse().unwrap_or(Script::UNKNOWN),
                                                family,
                                            ));
                                        } else if let Ok(locale) = Language::parse(lang) {
                                            if let Some(scr) = locale
                                                .script()
                                                .and_then(|s| s.parse::<Script>().ok())
                                            {
                                                // Also fallback for the script on its own
                                                script_fallback.push((scr, family));
                                                if scr == Script::from_bytes(*b"Hans")
                                                    || scr == Script::from_bytes(*b"Hant")
                                                {
                                                    // Han without a matching locale uses
                                                    // the first configured Han family.
                                                    script_fallback.push((
                                                        Script::from_bytes(*b"Hani"),
                                                        family,
                                                    ));
                                                }
                                            }
                                            locale_fallback.push((locale, family));
                                        }
                                    }
                                }

                                // family-specific fallback families, currently unimplemented
                                // because it requires a GenericFamily to be plumbed through
                                // the `RangedStyle` `font_stack` from `resolve` where it is
                                // currently thrown away.
                                {}
                            }
                            // TODO: interpret variant="compact" without fallbackFor as a
                            //       fallback for system-ui, as falling back to a
                            //       variant="elegant" for system-ui can mess up a layout
                            //       in some scripts.
                        }
                        _ => {}
                    }
                }
            }
        }

        Self {
            name_map: Arc::new(name_map),
            generic_families: Arc::new(generic_families),
            family_map,
            locale_fallback: locale_fallback.into(),
            script_fallback: script_fallback.into(),
        }
    }

    pub(crate) fn family(&self, id: FamilyId) -> Option<FamilyInfo> {
        self.family_map.get(&id).cloned()
    }

    pub(crate) fn fallback(&self, key: impl Into<FallbackKey>) -> Option<FamilyId> {
        let key: FallbackKey = key.into();
        let script = key.script();

        key.locale()
            .and_then(|locale| {
                if script == Script::from_bytes(*b"Hani") {
                    // FallbackKey canonicalizes requests (e.g. zh-Hant ->
                    // zh-TW), so normalize the XML tags by the same rules.
                    let find = |locale| {
                        self.locale_fallback
                            .iter()
                            .find(|(other, _)| han_locale(*other) == Some(locale))
                            .map(|(_, fid)| *fid)
                    };
                    find(locale).or_else(|| {
                        // Prefer an exact regional family above; when absent,
                        // use the configured simplified/traditional family.
                        let equivalent = match locale.as_str() {
                            "zh-HK" | "zh-MO" => "zh-TW",
                            "zh-SG" => "zh-CN",
                            _ => return None,
                        };
                        find(Language::parse(equivalent).ok()?)
                    })
                } else {
                    self.locale_fallback
                        .iter()
                        .find(|(other, _)| locale == *other)
                        .map(|(_, fid)| *fid)
                }
            })
            .or_else(|| {
                self.script_fallback
                    .iter()
                    .find(|(s, _)| script == *s)
                    .map(|(_, fid)| *fid)
            })
            .or_else(|| {
                self.generic_families
                    .get(GenericFamily::SansSerif)
                    .first()
                    .copied()
            })
    }
}

fn han_locale(locale: Language) -> Option<Language> {
    match locale.language() {
        "ja" | "ko" => {}
        "zh" if locale
            .script()
            .is_none_or(|script| matches!(script, "Hans" | "Hant")) => {}
        _ => return None,
    }
    FallbackKey::new(Script::from_bytes(*b"Hani"), Some(&locale)).locale()
}

#[cfg(test)]
#[path = "android_tests.rs"]
mod tests;
