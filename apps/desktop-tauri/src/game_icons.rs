use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{Map, Value};

const ICON_SIZE: i32 = 48;
const GAMES: [&str; 3] = ["factorio", "minecraft-java", "minecraft-bedrock"];

pub fn load(app: &tauri::AppHandle, executables: &Value) -> Value {
    platform::load(app, executables)
}

fn icons(executables: &Value, find: impl Fn(&str, Option<&str>) -> Option<Vec<u8>>) -> Value {
    let entries: Map<String, Value> = GAMES
        .into_iter()
        .filter_map(|game| {
            let png = find(game, executables[game].as_str())?;
            Some((
                game.into(),
                format!("data:image/png;base64,{}", STANDARD.encode(png)).into(),
            ))
        })
        .collect();
    Value::Object(entries)
}

#[cfg(not(windows))]
mod platform {
    use super::{icons, Value, GAMES, ICON_SIZE};
    use gtk::{
        gdk_pixbuf::Pixbuf,
        gio::{prelude::AppInfoExt, DesktopAppInfo},
        prelude::IconThemeExt,
        IconLookupFlags, IconTheme,
    };
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::mpsc,
    };

    pub fn load(app: &tauri::AppHandle, executables: &Value) -> Value {
        let (send, recv) = mpsc::channel();
        let use_prism = executables["minecraft-java"]
            .as_str()
            .and_then(|path| Path::new(path).file_stem())
            .is_some_and(|name| name == "prismlauncher");
        // GTK's icon theme belongs to the UI thread. Decode files on the worker.
        let _ = app.run_on_main_thread(move || {
            let paths = IconTheme::default().map(|theme| {
                GAMES
                    .into_iter()
                    .map(|game| (game, desktop_icon_paths(&theme, game, use_prism)))
                    .collect::<HashMap<_, _>>()
            });
            let _ = send.send(paths.unwrap_or_default());
        });
        let paths = recv.recv().unwrap_or_default();
        icons(executables, |game, executable| {
            let configured = (game == "factorio")
                .then_some(executable)
                .flatten()
                .into_iter()
                .flat_map(factorio_icon_paths);
            configured
                .chain(paths.get(game).into_iter().flatten().cloned())
                .find_map(|path| {
                    Pixbuf::from_file_at_scale(path, ICON_SIZE, ICON_SIZE, true)
                        .ok()?
                        .save_to_bufferv("png", &[])
                        .ok()
                })
        })
    }

    fn desktop_icon_paths(theme: &IconTheme, game: &str, use_prism: bool) -> Vec<PathBuf> {
        let ids: &[&str] = match game {
            "factorio" => &[
                "Factorio.desktop",
                "factorio.desktop",
                "steam_app_427520.desktop",
            ],
            "minecraft-java" if use_prism => &[
                "org.prismlauncher.PrismLauncher.desktop",
                "prismlauncher.desktop",
                "minecraft-launcher.desktop",
                "com.mojang.Minecraft.desktop",
            ],
            "minecraft-java" => &[
                "minecraft-launcher.desktop",
                "com.mojang.Minecraft.desktop",
                "org.prismlauncher.PrismLauncher.desktop",
                "prismlauncher.desktop",
            ],
            "minecraft-bedrock" => &[
                "io.mrarm.mcpelauncher.desktop",
                "mcpelauncher-ui-qt.desktop",
            ],
            _ => &[],
        };
        ids.iter()
            .filter_map(|id| {
                let icon = DesktopAppInfo::new(id)?.icon()?;
                theme
                    .lookup_by_gicon(&icon, ICON_SIZE, IconLookupFlags::FORCE_SIZE)?
                    .filename()
            })
            .collect()
    }

    fn factorio_icon_paths(executable: &str) -> impl Iterator<Item = PathBuf> + '_ {
        let path = Path::new(executable);
        let suffix = Path::new("bin/x64/factorio");
        path.canonicalize()
            .ok()
            .filter(|resolved| resolved != path)
            .into_iter()
            .chain([path.to_path_buf()])
            .filter_map(move |path| {
                path.ends_with(suffix).then_some(())?;
                Some(
                    path.parent()?
                        .parent()?
                        .parent()?
                        .join("data/core/graphics/factorio.png"),
                )
            })
    }

    #[cfg(test)]
    #[test]
    fn follows_factorio_launcher_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("bin/x64/factorio");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::File::create(&executable).unwrap();
        let artwork = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("data/core/graphics/factorio.png");
        for alias in ["factorio", "alias/bin/x64/factorio"] {
            let link = dir.path().join(alias);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&executable, &link).unwrap();
            assert_eq!(
                factorio_icon_paths(link.to_str().unwrap())
                    .next()
                    .as_deref(),
                Some(artwork.as_path())
            );
        }
        let alias_artwork = dir.path().join("alias/data/core/graphics/factorio.png");
        std::fs::create_dir_all(alias_artwork.parent().unwrap()).unwrap();
        std::fs::write(&alias_artwork, include_bytes!("../icons/icon.png")).unwrap();
        let alias = dir.path().join("alias/bin/x64/factorio");
        let icon = factorio_icon_paths(alias.to_str().unwrap())
            .find(|file| Pixbuf::from_file_at_scale(file, ICON_SIZE, ICON_SIZE, true).is_ok());
        assert_eq!(icon, Some(alias_artwork));
    }
}

#[cfg(windows)]
mod platform {
    use super::{icons, Value, ICON_SIZE};
    use std::{os::windows::ffi::OsStrExt, path::Path};
    use windows::{
        core::{HSTRING, PCWSTR},
        Win32::{
            Foundation::SIZE,
            Graphics::Gdi::{
                DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
                BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
            },
            System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            UI::Shell::{
                ExtractIconExW, FOLDERID_AppsFolder, IShellItemImageFactory,
                SHCreateItemFromParsingName, SHCreateItemInKnownFolder, KF_FLAG_DEFAULT,
                SIIGBF_ICONONLY,
            },
        },
    };

    pub fn load(_app: &tauri::AppHandle, executables: &Value) -> Value {
        // Each blocking worker initializes and releases its own COM apartment.
        if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
            return Value::Null;
        }
        let result = icons(executables, |game, executable| {
            executable
                .and_then(|path| exe_icon(Path::new(path)))
                .or_else(|| installed_icon(game))
        });
        unsafe { CoUninitialize() };
        result
    }

    fn installed_icon(game: &str) -> Option<Vec<u8>> {
        let (relative_paths, app_ids): (&[&str], &[&str]) = match game {
            "factorio" => (
                &[
                    r"Steam\steamapps\common\Factorio\bin\x64\factorio.exe",
                    r"GOG Galaxy\Games\Factorio\bin\x64\factorio.exe",
                    r"Factorio\bin\x64\factorio.exe",
                ],
                &[],
            ),
            "minecraft-java" => (
                &[r"Minecraft Launcher\MinecraftLauncher.exe"],
                &["Microsoft.4297127D64EC6_8wekyb3d8bbwe!Minecraft"],
            ),
            "minecraft-bedrock" => (
                &[],
                &[
                    "Microsoft.MinecraftUWP_8wekyb3d8bbwe!Game",
                    "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App",
                ],
            ),
            _ => return None,
        };
        for root in ["ProgramFiles(x86)", "ProgramFiles"]
            .into_iter()
            .filter_map(std::env::var_os)
        {
            for relative in relative_paths {
                if let Some(icon) = exe_icon(&Path::new(&root).join(relative)) {
                    return Some(icon);
                }
            }
        }
        app_ids.iter().find_map(|&id| {
            let name = HSTRING::from(id);
            let factory = unsafe {
                SHCreateItemInKnownFolder(
                    &FOLDERID_AppsFolder,
                    KF_FLAG_DEFAULT,
                    PCWSTR(name.as_ptr()),
                )
            }
            .ok()?;
            shell_icon(factory)
        })
    }

    fn exe_icon(path: &Path) -> Option<Vec<u8>> {
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            || !path.is_file()
        {
            return None;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let name = PCWSTR(wide.as_ptr());
        // Count embedded resources first so the generic Windows executable
        // icon never takes precedence over the game's installed icon.
        let count = unsafe { ExtractIconExW(name, -1, None, None, 0) };
        if count == 0 || count == u32::MAX {
            return None;
        }
        let factory = unsafe { SHCreateItemFromParsingName(name, None) }.ok()?;
        shell_icon(factory)
    }

    fn shell_icon(factory: IShellItemImageFactory) -> Option<Vec<u8>> {
        let size = SIZE {
            cx: ICON_SIZE,
            cy: ICON_SIZE,
        };
        let bitmap = unsafe { factory.GetImage(size, SIIGBF_ICONONLY) }.ok()?;
        let png = bitmap_png(bitmap);
        let _ = unsafe { DeleteObject(bitmap.into()) };
        png
    }

    fn bitmap_png(bitmap: HBITMAP) -> Option<Vec<u8>> {
        let mut dimensions = BITMAP::default();
        if unsafe {
            GetObjectW(
                bitmap.into(),
                std::mem::size_of::<BITMAP>() as i32,
                Some((&mut dimensions as *mut BITMAP).cast()),
            )
        } == 0
        {
            return None;
        }
        let (width, height) = (dimensions.bmWidth, dimensions.bmHeight);
        if !(1..=ICON_SIZE).contains(&width) || !(1..=ICON_SIZE).contains(&height) {
            return None;
        }
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = unsafe { GetDC(None) };
        if dc.is_invalid() {
            return None;
        }
        let lines = unsafe {
            GetDIBits(
                dc,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        unsafe { ReleaseDC(None, dc) };
        if lines != height {
            return None;
        }
        // Shell bitmaps use premultiplied BGRA. An unused alpha channel
        // (older opaque icons) is returned as all zeroes.
        let has_alpha = pixels.chunks_exact(4).any(|pixel| pixel[3] != 0);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            if !has_alpha {
                pixel[3] = 255;
            } else if (1..255).contains(&pixel[3]) {
                let alpha = u32::from(pixel[3]);
                for color in &mut pixel[..3] {
                    *color = (u32::from(*color) * 255 / alpha).min(255) as u8;
                }
            }
        }
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .ok()?
            .write_image_data(&pixels)
            .ok()?;
        Some(bytes)
    }
}
