//! Finding and loading application icons, from desktop entries and icon themes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{env, fs};

use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::Transform;

use crate::render_helpers::memory::MemoryBuffer;

/// Icon size the theme directories are ranked by, in pixels.
const WANTED_SIZE: i32 = 64;

/// Where to look for icons.
#[derive(Debug, Clone, Default)]
pub struct IconSearch {
    /// Directories holding `applications/` with desktop entries.
    pub data_dirs: Vec<PathBuf>,
    /// Directories holding icon themes.
    pub icon_dirs: Vec<PathBuf>,
    /// Directories holding loose icons, like `/usr/share/pixmaps`.
    pub pixmap_dirs: Vec<PathBuf>,
    /// The user's icon theme, if any; `hicolor` always comes last.
    pub theme: Option<String>,
}

impl IconSearch {
    /// Searches the XDG directories, with the GTK icon theme.
    pub fn from_env() -> Self {
        let home = env::var_os("HOME").map(PathBuf::from);
        let data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
        let data_dirs = env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| String::from("/usr/local/share:/usr/share"));

        let mut data: Vec<PathBuf> = data_home.into_iter().collect();
        data.extend(env::split_paths(&data_dirs));

        let mut icon_dirs: Vec<_> = data.iter().map(|dir| dir.join("icons")).collect();
        if let Some(home) = &home {
            icon_dirs.push(home.join(".icons"));
        }
        let mut pixmap_dirs: Vec<_> = data.iter().map(|dir| dir.join("pixmaps")).collect();
        pixmap_dirs.push(PathBuf::from("/usr/share/pixmaps"));

        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        let theme = config_home
            .and_then(|dir| fs::read_to_string(dir.join("gtk-3.0/settings.ini")).ok())
            .and_then(|ini| ini_value(&ini, "Settings", "gtk-icon-theme-name"));

        Self {
            data_dirs: data,
            icon_dirs,
            pixmap_dirs,
            theme,
        }
    }

    /// The name of the icon for an app id: from its desktop entry, or the app id itself.
    pub fn icon_name(&self, app_id: &str) -> String {
        let file = format!("{app_id}.desktop");
        let lower = file.to_lowercase();
        for dir in &self.data_dirs {
            let dir = dir.join("applications");
            let mut path = dir.join(&file);
            if !path.is_file() {
                // Fall back to a case-insensitive match, for app ids like "Firefox".
                let found = fs::read_dir(&dir).ok().and_then(|entries| {
                    entries.flatten().map(|entry| entry.path()).find(|p| {
                        p.file_name()
                            .is_some_and(|n| n.to_string_lossy().to_lowercase() == lower)
                    })
                });
                match found {
                    Some(found) => path = found,
                    None => continue,
                }
            }
            if let Some(icon) = fs::read_to_string(&path)
                .ok()
                .and_then(|entry| parse_desktop_icon(&entry))
            {
                return icon;
            }
        }
        app_id.to_owned()
    }

    /// Icon files for an icon name, best first.
    pub fn find(&self, name: &str) -> Vec<PathBuf> {
        let path = Path::new(name);
        if path.is_absolute() {
            return vec![path.to_owned()];
        }

        let mut rv = Vec::new();
        let mut seen = Vec::new();
        let mut themes: Vec<String> = self.theme.iter().cloned().collect();
        let mut i = 0;
        // Walk the theme and everything it inherits, breadth first, then hicolor.
        while i < themes.len() {
            let theme = themes[i].clone();
            i += 1;
            if seen.contains(&theme) || theme == "hicolor" {
                continue;
            }
            seen.push(theme.clone());
            for inherited in self.search_theme(&theme, name, &mut rv) {
                themes.push(inherited);
            }
        }
        self.search_theme("hicolor", name, &mut rv);

        for dir in &self.pixmap_dirs {
            for ext in ["svg", "png"] {
                let path = dir.join(format!("{name}.{ext}"));
                if path.is_file() {
                    rv.push(path);
                }
            }
        }
        rv
    }

    /// Adds icon files from one theme, best first, and returns the themes it inherits.
    fn search_theme(&self, theme: &str, name: &str, rv: &mut Vec<PathBuf>) -> Vec<String> {
        let mut inherits = Vec::new();
        for base in &self.icon_dirs {
            let root = base.join(theme);
            let Ok(index) = fs::read_to_string(root.join("index.theme")) else {
                continue;
            };
            if inherits.is_empty() {
                if let Some(value) = ini_value(&index, "Icon Theme", "Inherits") {
                    inherits = value.split(',').map(|s| s.trim().to_owned()).collect();
                }
            }

            let mut dirs = theme_dirs(&index);
            dirs.sort_by_key(|dir| dir.rank());
            for dir in dirs {
                for ext in ["svg", "png"] {
                    let path = root.join(&dir.path).join(format!("{name}.{ext}"));
                    if path.is_file() {
                        rv.push(path);
                    }
                }
            }
        }
        inherits
    }
}

/// A directory of an icon theme.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeDir {
    path: String,
    size: i32,
    scalable: bool,
}

impl ThemeDir {
    /// Lower is better: scalable first, then the smallest at least the wanted size, then the
    /// largest below it, since scaling down looks better than scaling up.
    fn rank(&self) -> (bool, bool, i32) {
        (
            !self.scalable,
            self.size < WANTED_SIZE,
            (self.size - WANTED_SIZE).abs(),
        )
    }
}

/// The directories of a theme from its `index.theme`, without symbolic icons.
fn theme_dirs(index: &str) -> Vec<ThemeDir> {
    let Some(dirs) = ini_value(index, "Icon Theme", "Directories") else {
        return Vec::new();
    };
    dirs.split(',')
        .map(str::trim)
        .filter(|dir| !dir.is_empty() && !dir.contains("symbolic"))
        .map(|dir| {
            let size = ini_value(index, dir, "Size")
                .and_then(|size| size.parse().ok())
                .unwrap_or(0);
            let scalable = ini_value(index, dir, "Type").as_deref() == Some("Scalable")
                || dir.contains("scalable");
            ThemeDir {
                path: dir.to_owned(),
                size,
                scalable,
            }
        })
        .collect()
}

/// A value from an ini-style file.
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name == section;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().to_owned());
            }
        }
    }
    None
}

/// The `Icon=` of a desktop entry.
pub fn parse_desktop_icon(entry: &str) -> Option<String> {
    ini_value(entry, "Desktop Entry", "Icon").filter(|icon| !icon.is_empty())
}

/// Turns rows of straight RGB(A) pixels into premultiplied BGRA, as `Argb8888` is laid out.
pub fn rgba_to_premul_bgra(
    pixels: &[u8],
    width: usize,
    height: usize,
    rowstride: usize,
    has_alpha: bool,
) -> Vec<u8> {
    let channels = if has_alpha { 4 } else { 3 };
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &pixels[y * rowstride..];
        for x in 0..width {
            let px = &row[x * channels..x * channels + channels];
            let a = if has_alpha { px[3] } else { 255 };
            let premul = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
            out.extend_from_slice(&[premul(px[2]), premul(px[1]), premul(px[0]), a]);
        }
    }
    out
}

/// Loads the first of these icon files that works, at most `px` pixels on a side.
pub fn load_icon(paths: &[PathBuf], px: i32, scale: f64) -> Option<MemoryBuffer> {
    paths.iter().find_map(|path| {
        let pixbuf = match gdk_pixbuf::Pixbuf::from_file_at_scale(path, px, px, true) {
            Ok(pixbuf) => pixbuf,
            Err(err) => {
                debug!("error loading icon {path:?}: {err}");
                return None;
            }
        };
        let width = usize::try_from(pixbuf.width()).ok()?;
        let height = usize::try_from(pixbuf.height()).ok()?;
        let rowstride = usize::try_from(pixbuf.rowstride()).ok()?;
        let bytes = pixbuf.read_pixel_bytes();
        let data = rgba_to_premul_bgra(&bytes, width, height, rowstride, pixbuf.has_alpha());
        Some(MemoryBuffer::new(
            data,
            Fourcc::Argb8888,
            (pixbuf.width(), pixbuf.height()),
            scale,
            Transform::Normal,
        ))
    })
}

/// Icon files by app id, looked up once each.
#[derive(Debug, Default)]
pub struct AppIcons {
    search: RefCell<Option<IconSearch>>,
    paths: RefCell<HashMap<String, Vec<PathBuf>>>,
}

impl AppIcons {
    pub fn new() -> Self {
        Self::default()
    }

    /// Icon files for an app id, best first, empty if there are none.
    pub fn paths(&self, app_id: &str) -> Vec<PathBuf> {
        if let Some(paths) = self.paths.borrow().get(app_id) {
            return paths.clone();
        }
        let mut search = self.search.borrow_mut();
        let search = search.get_or_insert_with(IconSearch::from_env);
        let paths = search.find(&search.icon_name(app_id));
        self.paths
            .borrow_mut()
            .insert(app_id.to_owned(), paths.clone());
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("niri-app-icon-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn desktop_icon_is_read_from_the_entry_section() {
        let entry = "[Desktop Entry]\nName=Foot\nIcon=foot\n\n[Desktop Action new]\nIcon=other\n";
        assert_eq!(parse_desktop_icon(entry).as_deref(), Some("foot"));
        assert_eq!(parse_desktop_icon("[Desktop Entry]\nName=x\n"), None);
        assert_eq!(parse_desktop_icon("[Desktop Entry]\nIcon=\n"), None);
    }

    #[test]
    fn icon_name_comes_from_the_desktop_entry() {
        let dir = temp_dir("entry");
        fs::create_dir_all(dir.join("applications")).unwrap();
        fs::write(
            dir.join("applications/org.example.App.desktop"),
            "[Desktop Entry]\nIcon=example\n",
        )
        .unwrap();
        let search = IconSearch {
            data_dirs: vec![dir.clone()],
            ..Default::default()
        };
        assert_eq!(search.icon_name("org.example.App"), "example");
        // Case-insensitively too.
        assert_eq!(search.icon_name("org.example.app"), "example");
        // Without an entry, the app id is the icon name.
        assert_eq!(search.icon_name("foot"), "foot");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn themes_are_searched_scalable_first_then_inherited_then_hicolor() {
        let dir = temp_dir("theme");
        let icons = dir.join("icons");
        fs::create_dir_all(icons.join("Mine")).unwrap();
        fs::write(
            icons.join("Mine/index.theme"),
            "[Icon Theme]\nInherits=Base\nDirectories=apps/22,apps/scalable,apps/symbolic\n\n\
             [apps/22]\nSize=22\n\n[apps/scalable]\nSize=64\nType=Scalable\n",
        )
        .unwrap();
        touch(&icons.join("Mine/apps/22/app.png"));
        touch(&icons.join("Mine/apps/scalable/app.svg"));
        touch(&icons.join("Mine/apps/symbolic/app.svg"));

        fs::create_dir_all(icons.join("Base")).unwrap();
        fs::write(
            icons.join("Base/index.theme"),
            "[Icon Theme]\nDirectories=48\n\n[48]\nSize=48\n",
        )
        .unwrap();
        touch(&icons.join("Base/48/app.png"));

        fs::create_dir_all(icons.join("hicolor")).unwrap();
        fs::write(
            icons.join("hicolor/index.theme"),
            "[Icon Theme]\nDirectories=16x16/apps,128x128/apps\n\n\
             [16x16/apps]\nSize=16\n\n[128x128/apps]\nSize=128\n",
        )
        .unwrap();
        touch(&icons.join("hicolor/16x16/apps/app.png"));
        touch(&icons.join("hicolor/128x128/apps/app.png"));
        touch(&dir.join("pixmaps/app.png"));

        let search = IconSearch {
            icon_dirs: vec![icons.clone()],
            pixmap_dirs: vec![dir.join("pixmaps")],
            theme: Some(String::from("Mine")),
            ..Default::default()
        };
        let found: Vec<_> = search
            .find("app")
            .into_iter()
            .map(|path| path.strip_prefix(&dir).unwrap().to_owned())
            .collect();
        let expected: Vec<PathBuf> = [
            "icons/Mine/apps/scalable/app.svg",
            "icons/Mine/apps/22/app.png",
            "icons/Base/48/app.png",
            "icons/hicolor/128x128/apps/app.png",
            "icons/hicolor/16x16/apps/app.png",
            "pixmaps/app.png",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(found, expected);

        assert!(search.find("missing").is_empty());
        assert_eq!(
            search.find("/abs/icon.png"),
            [PathBuf::from("/abs/icon.png")]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pixels_are_premultiplied_and_swizzled() {
        // One row of two RGBA pixels, padded to a rowstride of 12.
        let pixels = [255, 0, 0, 255, 0, 255, 0, 128, 9, 9, 9, 9];
        assert_eq!(
            rgba_to_premul_bgra(&pixels, 2, 1, 12, true),
            [0, 0, 255, 255, 0, 128, 0, 128]
        );
        // RGB without alpha is opaque.
        let pixels = [10, 20, 30];
        assert_eq!(
            rgba_to_premul_bgra(&pixels, 1, 1, 3, false),
            [30, 20, 10, 255]
        );
    }
}
