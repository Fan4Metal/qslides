<p align="center">
  <img src="images/icon.png" width="128" height="128" alt="qslides icon">
</p>

<h1 align="center">qslides</h1>

<p align="center">A desktop photo frame that shows pictures and silent videos from your folders</p>

<p align="center">
  <a href="https://github.com/Fan4Metal/qslides/releases/latest"><img src="https://img.shields.io/github/v/release/Fan4Metal/qslides?label=release" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/Windows-10%2B-0078D6" alt="Windows 10 or later">
  <img src="https://img.shields.io/badge/Rust-B7410E?logo=rust&logoColor=white" alt="Rust">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center"><b>English</b> | <a href="README.ru.md">Русский</a></p>

qslides is a small widget for the Windows desktop that works as a photo frame. Several folders are added to its library, and the widget shows a slideshow of the pictures and videos found in them and in all their subfolders. Videos play without sound. The window has no frame or title bar: it is moved by dragging the picture, sized by its edges, and controlled through the menu of the right mouse button. Pictures and videos are decoded by the codecs of Windows itself, so the program consists of a single executable file.

## Features

- **A library of folders**: any number of folders, scanned with all their subfolders (see [Library](#library)).
- **Pictures and videos**: JPEG, PNG, GIF, BMP, TIFF, JPEG XR, and HEIC, AVIF and WebP when their extensions from the Microsoft Store are installed; MP4, MOV, MKV, AVI, WMV, WebM and other videos that Windows plays, without sound.
- **Slideshow settings**: the time each slide stays (1 second to an hour), the order (by name, by date, random), a picture fitted whole or filling the window, videos played to the end or as long as pictures (see [Slideshow](#slideshow)).
- **A frameless window** with presets of its proportions, from 16:9 to 9:16, kept while the window is sized.
- **Start with Windows, always on top, only in the tray**: each is one switch of the menu.
- **Quiet**: between slides the program does no work; the next slide is decoded in advance, and slides change with a short cross-fade.
- **English or Russian interface**: it follows the language of Windows and can be changed in the About window (`F1`).
- **Portable**: the [portable archive](#installation) keeps its settings next to the program.
- **Privacy**: no network access beyond the optional update check (see [Update check and privacy](#update-check-and-privacy)).

## Installation

The installer and the portable archive are on the [Releases](https://github.com/Fan4Metal/qslides/releases) page.

The installer, `qslides_<version>_Setup.exe`, needs no administrator rights: the program is placed in `%LOCALAPPDATA%\Programs\qslides`. The installed program keeps its settings in `%APPDATA%\qslides`. Uninstallation also removes the program from the start-up list of Windows.

The portable archive, `qslides_<version>_portable.zip`, holds the program in a `qslides` folder and runs without installation. It also holds an empty `app.ron`. While that file is next to `qslides.exe` and the folder is writable, the settings and the library are kept beside it, so the program can be carried on a removable drive. Without the file, the program keeps them in `%APPDATA%\qslides`, as the installed program does. The About window shows where the settings are kept.

The executables are not code-signed, so Windows SmartScreen may warn about an unknown publisher on first launch.

## Usage

The window shows the current slide on a black background. Dragging the picture moves the window, dragging its edges and corners sizes it. When the pointer moves over the window, the **Previous**, **Pause** and **Next** buttons appear at its bottom, and a cross that closes the program at its top right corner; they fade out again after a few seconds. A double click opens the file shown in the program Windows associates with it.

Everything else is in the menu of the right mouse button, which is also opened by the right button on the tray icon. The menu is a small panel in a window of its own, so it extends beyond the edges of a small widget; near the edges of the screen it opens to the left or upward. It closes on a click elsewhere or on `Esc`; the settings in it take effect at once and leave it open.

| Part | What it does |
|---|---|
| Buttons at the top | **Previous**, **Pause**, **Next** and the number of the file shown out of all the files of the show (`12 / 340`); **Open the file** in its program, **Show in its folder** in Explorer |
| **Show each for** | A slider for the time a slide stays, from 1 second to 1 hour |
| **Order** | **By name**, **By date**, **Random** |
| **Picture** | **Fit whole** or **Fill the window** |
| **Videos** | **To the end** or **As long as pictures** |
| **Window proportions** | **Any**, 16:9, 16:10, 3:2, 4:3, 5:4, 1:1, 4:5, 3:4, 2:3, 9:16 |
| **Folders** | The library: each folder with buttons to open it in Explorer and to remove it, **Add a folder…**, **Look for new files** |
| **Always on top** | The window stays above other windows |
| **Start with Windows** | The program starts when the user signs in |
| **Only in the tray** | No button on the taskbar; an icon in the notification area instead |
| **About qslides** | Version, language, update check |

Only one copy of the program runs at a time: starting it again brings the window already open to the front.

### Library

Folders are added with **Folders → Add a folder…** (`Ctrl+O`) or by dropping them on the window; a file dropped on the window adds its folder. The library is scanned at start-up, after each change and with **Look for new files** (`F5`). Hidden and system folders, links and junctions are skipped.

<details><summary>Where the library is kept</summary>

The folders are listed one per line in `library.txt`, next to the settings file: in `%APPDATA%\qslides\data` for the installed program, next to `qslides.exe` for the portable one. The file shown when the program is closed is remembered, and the show goes on from it at the next start.

</details>

### Slideshow

**By name** shows the folders in the order they were added to the library and, inside each, the files and subfolders by name as Explorer sorts them: `2.jpg` before `10.jpg`. **By date** goes from the oldest file to the newest by the time it was last changed. **Random** shows every file once per round in a shuffled order and shuffles anew for the next round.

A picture is scaled down to the size of the screen when it is decoded and turned upright by its EXIF orientation. **Fit whole** shows all of it with black bars where its proportions differ from the window's; **Fill the window** covers the window and cuts the edges of the picture.

A video is shown either to its end, however long it is (**Videos: To the end**), or for the same time as a picture (**Videos: As long as pictures**): a longer one is cut off, a shorter one starts again from the beginning. The sound of a video is never played. Videos are decoded on the graphics card when Windows can, so 4K videos of a phone play smoothly; a slide shows at most 30 frames a second. A file that cannot be decoded is skipped until the next scan.

<details><summary>Which formats need extensions from the Microsoft Store</summary>

Windows decodes JPEG, PNG, GIF (the first frame), BMP, TIFF, ICO and JPEG XR by itself, and H.264, MPEG-4 and WMV videos. HEIC needs the HEIF Image Extensions and the HEVC Video Extensions; AVIF needs the AV1 Video Extension; WebP needs the WebP Image Extensions; VP9 videos need the VP9 Video Extensions. Files Windows cannot decode are skipped.

</details>

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Right`, `Page Down`, wheel down | Next slide |
| `Left`, `Page Up`, wheel up | Previous slide |
| `Space` | Pause or go on |
| `Enter`, double click | Open the file shown |
| `Ctrl+O` | Add a folder |
| `F5` | Look for new files |
| `F1` | About |

## Update check and privacy

The update check is optional and off by default. With **Check for updates at start-up (once a day)** enabled in the About window, the program asks the GitHub Releases API (`api.github.com`) for the latest release at most once a day; **Check now** in the same window asks at once. The request carries nothing but the program version in its `User-Agent` header, and nothing is downloaded or installed: when a newer release exists, an item with its version appears at the top of the menu and opens the release page in the browser. Otherwise the program does not access the network.

## Building

Requirements: Windows 10 or later, a stable Rust toolchain (MSVC target, 2024 edition) and the Visual Studio 2022 Build Tools with the C++ workload. For `build.py`, [uv](https://docs.astral.sh/uv/).

```
uv run build.py          # target\release\qslides.exe
cargo test
```

`build.py` first closes a `qslides.exe` running from the project's `target` folder, because it would lock the file, then runs `cargo build --release`. When the program is not running, plain `cargo build --release` works as well. The executable links the C runtime statically (`.cargo/config.toml`), so it needs nothing beyond Windows itself.

## Installer and release

The release script builds the executable, the installer and the portable archive. It needs [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```
python tools/make_release.py              # tests, release build, installer, archive
python tools/make_release.py --no-tests   # the same without cargo test
python tools/make_release.py --install    # then a silent installation over the installed copy
```

The script writes the files to `dist` and takes the version from `Cargo.toml`. The installer script is `tools/setup.iss`; `qslides.exe --export-icon <file.ico>` writes the installer icon.

Releases on GitHub are built by the **Release** workflow (`.github/workflows/release.yml`). After the version in `Cargo.toml` is updated and committed, pushing a matching tag publishes a release with the installer and the portable archive:

```
git tag v0.1.0
git push origin v0.1.0
```

The workflow stops if the tag does not match the version in `Cargo.toml`. Running the workflow manually (Actions → Release → Run workflow) builds the files and attaches them to the run, without making a release.

## Built with

qslides is built on the following open-source projects:

- [egui / eframe](https://github.com/emilk/egui): the user interface and the window;
- [windows-rs](https://github.com/microsoft/windows-rs): Windows Imaging Component, Media Foundation and the rest of the Windows API;
- [rfd](https://github.com/PolyMeilex/rfd): the folder dialog;
- [Inno Setup](https://jrsoftware.org/isinfo.php): the installer.

## License

MIT, see [LICENSE](LICENSE).
