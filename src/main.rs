use clap::Parser;
use rand::seq::SliceRandom;
use std::{
    fs::DirEntry,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

#[cfg(target_os = "linux")]
const FLATPAK_APPLICATIONS_PATH: &str = ".var/app/com.valvesoftware.Steam/data/Steam";
#[cfg(target_os = "linux")]
const VANILLA_APPLICATIONS_PATHS: [&str; 2] = [r#".local/share/steam"#, r#".steam/steam"#];
#[cfg(target_os = "windows")]
const VANILLA_APPLICATIONS_PATH: &str = r#"C:\Program Files (x86)\Steam"#;
#[cfg(target_os = "macos")]
const VANILLA_APPLICATIONS_PATH: &str = r#"Library/Application Support/Steam"#;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const MANIFEST_DIR: &str = "steamapps/";

/// Builds the appropriate url to run the game
fn generate_steam_rungame(id: &str) -> String {
    format!("steam://rungameid/{}", id)
}

/// Detect if app is a Proton runtime
fn is_proton(app_name: &str) -> bool {
    if app_name.starts_with("Proton") {
        let number = app_name.split(' ').collect::<Vec<&str>>();
        if number.len() > 1 {
            let number = number[1].parse::<f32>();
            if number.unwrap_or(0.0) != 0.0 {
                return true;
            }
        }
    }
    false
}

/// List of names of applications/games we don't want to launch.
fn is_blacklisted(app_name: &str) -> bool {
    let steam_libs = [
        "Steamworks Common Redistributables",
        "SteamVR",
        "Proton Experimental",
    ];

    steam_libs.iter().any(|&b| b == app_name)
	|| app_name.ends_with("Soundtrack") // This **should** deal with downloaded albums, and ignore them
	|| is_proton(app_name)
	|| app_name.starts_with("Steam Linux Runtime")
}

// fn parse_vdf(path_to_vdf: &Path) -> HashMap<String, String> {
//     let mut res = HashMap::new();

//     let contents = std::fs::read_to_string(path_to_vdf);
//     let lines = contents.unwrap();
//     let lines:Vec<&str> = lines.lines().collect();

//     for line in 0..lines.len() {
//         println!("Working on {:?}", lines[line]);
//         if line+1 < lines.len() && lines[line+1].trim() == "{" {

//         } else{
//             let key_value:Vec<&str> = lines[line].split_whitespace().collect();
//             if key_value.len() < 2 {
//                 continue;
//             }
//             println!("splitted {:?}", key_value);
//             res.insert(key_value[0].to_string(), key_value[1].to_string());

//         }
//     }

//     res
// }

/// Find other install directories which are not the default one
fn get_other_install_dirs(path: &Path) -> Vec<String> {
    let mut path = path.to_path_buf();
    path.push("libraryfolders.vdf");

    let path = path.as_path();

    let contents = std::fs::read_to_string(path);
    let lines = contents.unwrap();

    let mut libs = Vec::new();

    let lines = lines.lines();
    for line in lines {
        if line.contains("path") {
            let splitted: Vec<&str> = line.split_whitespace().collect();
            libs.push(splitted[1][1..splitted[1].len() - 1].to_string());
        }
    }
    libs
}

// Parse manifest and get list of game names with their ids.
fn get_games_from_manifest_in_path(path: &Path) -> Vec<(String, String)> {
    let dir = {
        match std::fs::read_dir(path) {
            Ok(path) => path,
            Err(_) => {
                // sometimes steam can have a corrupted library path, this is
                // probably fine since it only appeared for paths not in use for
                // me. Skip library and hope this is fine.
                return Vec::new();
            }
        }
    };

    let manifest_files = dir
        .map(|e| e.unwrap())
        .filter(|file| {
            let file_name = file.file_name();
            let file_name = file_name.to_str().unwrap();
            file_name.starts_with("appmanifest")
        })
        .collect::<Vec<DirEntry>>();

    let mut games = Vec::new();

    for file in manifest_files {
        let file_path = file.path();
        let contents = std::fs::read_to_string(file_path);
        let lines = contents.unwrap();
        let lines = lines.lines().skip(2).collect::<Vec<&str>>();

        let mut game = "".to_string();
        let mut id = "".to_string();

        if lines.is_empty() {
            // sometimes manifest files are empty or corrupted, skip them
            return games;
        }

        for line in lines.iter().take(lines.len() - 1) {
            let line = line
                .split('\t')
                .filter(|s| !s.is_empty())
                .collect::<Vec<&str>>();
            if line[0].contains("name") {
                let app_name = line[1].replace('\"', "");

                game = app_name.clone();
            }

            if line[0].contains("appid") {
                let app_id = line[1].replace('\"', "");
                id = app_id.clone();
            }
        }
        if !is_blacklisted(&game) {
            games.push((game, id));
        }
    }

    games
}

#[derive(Debug, PartialEq)]
enum SteamKind {
    Vanilla,
    AltPath(PathBuf),
    #[cfg(target_os = "linux")]
    Flatpak,
    NotFound,
}

/// Detect if Steam is installed.
#[cfg(target_os = "linux")]
fn detect_steam() -> SteamKind {
    let has_steam_vanilla = which::which("steam").is_ok();
    let flatpak_list = Command::new("flatpak")
        .arg("list")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut flatpak_steam_output = Command::new("grep")
        .stdin(flatpak_list.stdout.unwrap())
        .arg("-c")
        .arg("Steam")
        .output()
        .unwrap()
        .stdout;
    flatpak_steam_output.remove(flatpak_steam_output.len() - 1);
    let has_flatpak_steam = String::from_utf8(flatpak_steam_output).unwrap();

    let has_flatpak_steam = has_flatpak_steam.parse::<u32>().unwrap() > 0;

    match (has_steam_vanilla, has_flatpak_steam) {
        (true, _) => SteamKind::Vanilla,
        (_, true) => SteamKind::Flatpak,
        _ => SteamKind::NotFound,
    }
}

/// Detect if Steam is installed.
#[cfg(target_os = "windows")]
fn detect_steam() -> SteamKind {
    let has_steam_vanilla = which::which(r#"C:\Program Files (x86)\Steam\steam.exe"#).is_ok();
    if has_steam_vanilla {
        return SteamKind::Vanilla;
    }
    match get_steam_exe_path_from_reg() {
        Ok(binary_path) => {
            if which::which(binary_path.clone() + r#"\steam.exe"#).is_ok() {
                SteamKind::AltPath(binary_path.into())
            } else {
                eprintln!("steam.exe was not in install folder");
                eprintln!("expected path according to registry: {:?}", binary_path);
                SteamKind::NotFound
            }
        }
        Err(err) => {
            eprintln!("Couldn't find steam in registry due to error: {}", err);
            SteamKind::NotFound
        }
    }
}

#[cfg(target_os = "windows")]
/// Attempt to find steam's install location via the windows registry
fn get_steam_exe_path_from_reg() -> std::io::Result<String> {
    use winreg::enums::*;
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    let steam = hklm.open_subkey(r#"SOFTWARE\WOW6432Node\Valve\Steam"#)?;
    steam.get_value("InstallPath")
}

/// Detect if Steam is installed.
#[cfg(target_os = "macos")]
fn detect_steam() -> SteamKind {
    let has_steam_vanilla = which::which("steam").is_ok();
    match has_steam_vanilla {
        true => SteamKind::Vanilla,
        _ => SteamKind::NotFound,
    }
}

/// Launche the game from its id using the appropriate Steam environment
#[cfg(target_os = "linux")]
fn run(steam_type: SteamKind, id: &str) -> std::io::Result<Child> {
    let child = match steam_type {
        SteamKind::Flatpak => std::process::Command::new("flatpak")
            .args([
                "run",
                "com.valvesoftware.Steam",
                &generate_steam_rungame(id),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
        SteamKind::Vanilla => std::process::Command::new("steam")
            .arg(&generate_steam_rungame(id))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
        SteamKind::NotFound => panic!("Couldn't find steam!"),
    };
    println!("{:?} {} {:?}", steam_type, id, child);
    Ok(child)
}

/// Launch the game from its id using the appropriate Steam environment
#[cfg(target_os = "windows")]
fn run(steam_type: SteamKind, id: &str) -> std::io::Result<Child> {
    let binary_path: String = match steam_type {
        SteamKind::Vanilla => r#"C:\Program Files (x86)\Steam\steam.exe"#.into(),
        SteamKind::AltPath(binary_path) => binary_path
            .join("steam.exe")
            .into_os_string()
            .into_string()
            .unwrap(),
        _ => panic!("Couldn't find steam!"),
    };
    Command::new(&binary_path)
        .arg(&generate_steam_rungame(id))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Launche the game from its id using the appropriate Steam environment
#[cfg(target_os = "macos")]
fn run(steam_type: SteamKind, id: &str) -> std::io::Result<Child> {
    let child = match steam_type {
        SteamKind::Vanilla => std::process::Command::new("steam")
            .arg(&generate_steam_rungame(id))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
        SteamKind::NotFound => panic!("Couldn't find steam!"),
    };
    Ok(child)
}

/// Randomly picks an installed game from your Steam library and launches it.
#[derive(Parser)]
#[clap(
    version = VERSION
)]
struct Opts {
    /// Show short message telling which game is being launched
    #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Runs the program but doesn't launch the game.
    #[clap(short, long)]
    dry_run: bool,
}

fn main() {
    let opts: Opts = Opts::parse();

    let steam_type = detect_steam();

    if steam_type == SteamKind::NotFound {
        eprintln!("Couldn't find Steam. Please make sure it is installed.");
        return;
    }

    let mut path = {
        let mut home = dirs::home_dir().unwrap();
        match steam_type {
            #[cfg(target_os = "linux")]
            SteamKind::Flatpak => home.push(FLATPAK_APPLICATIONS_PATH),
            #[cfg(target_os = "linux")]
            SteamKind::Vanilla => home.push(
                VANILLA_APPLICATIONS_PATHS
                    .iter()
                    .find(|&&p| {
                        let mut test_path = home.to_path_buf();
                        test_path.push(p);
                        test_path.exists() && test_path.is_dir()
                    })
                    .unwrap(),
            ),
            #[cfg(not(target_os = "linux"))]
            SteamKind::Vanilla => home.push(VANILLA_APPLICATIONS_PATH),
            SteamKind::AltPath(ref path) => home = path.clone(),
            _ => {}
        }
        home
    };

    path.push(MANIFEST_DIR);

    let install_dirs = get_other_install_dirs(&path);

    let mut games = get_games_from_manifest_in_path(&path);

    for other_dir in install_dirs {
        let mut path = PathBuf::new();
        path.push(other_dir);
        path.push(MANIFEST_DIR);
        games.extend(get_games_from_manifest_in_path(&path));
    }

    let (game, id) = games.choose(&mut rand::thread_rng()).unwrap();

    if opts.verbose > 0 {
        println!("Randomly launching \"{}\"! Have fun!", game);
    }

    if !opts.dry_run {
        let _ = run(steam_type, id).unwrap();
    }
}
