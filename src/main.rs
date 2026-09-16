use anyhow::{Context, Result};
use inquire::{Select, Text};
use regex::Regex;
use std::{
    fs::{self, File},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};
use zip_extensions::{zip_create_from_directory, zip_extract};

const PROXIES_URL: &str =
    "https://raw.githubusercontent.com/Stefanuk12/RoProPatcher/master/proxies.txt";

// Cache compiled regex to avoid recompiling inside loops
static PATCH_REGEX: OnceLock<Regex> = OnceLock::new();

fn get_patch_regex() -> &'static Regex {
    PATCH_REGEX.get_or_init(|| {
        Regex::new(
            r#"(https://api\.)ropro\.io/(validateUser\.php|getServerInfo\.php|getServerConnectionScore\.php|getServerAge\.php|getSubscription\.php)"#,
        )
        .expect("Failed to compile patch regex")
    })
}

/// Fetches list of proxy endpoints asynchronously via reqwest 0.13.
async fn get_proxies() -> Result<Vec<String>> {
    let response = reqwest::get(PROXIES_URL)
        .await
        .context("Failed to request proxies list")?
        .text()
        .await
        .context("Failed to read proxies response text")?;

    let proxies: Vec<String> = response
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if proxies.is_empty() {
        anyhow::bail!("Proxy list downloaded but contained no valid entries");
    }

    Ok(proxies)
}

/// Modifies targeted API URLs inside JS files to redirect through the proxy.
fn patch_file(file_path: &Path, replacement: &str) -> Result<bool> {
    if !file_path.is_file() {
        return Ok(false);
    }

    let contents = fs::read_to_string(file_path)
        .with_context(|| format!("Unable to read file: {:?}", file_path))?;

    let re = get_patch_regex();
    let new_contents = re.replace_all(&contents, replacement).to_string();

    if contents != new_contents {
        fs::write(file_path, new_contents)
            .with_context(|| format!("Unable to write patched contents to: {:?}", file_path))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Performs patching across target extension directories.
fn patch_extension_dir(base_path: &Path, proxy: &str) -> Result<()> {
    let rep = format!("https://{}/${{2}}///api", proxy);

    // 1. Patch background.js
    let background_path = base_path.join("background.js");
    if background_path.exists() {
        if !patch_file(&background_path, &rep)? {
            println!("Warning: No changes made to `background.js` (already patched?).");
        }
    } else {
        println!("Warning: `background.js` not found in target path.");
    }

    // 2. Patch all JS files inside js/page directory
    let jspage_path = base_path.join("js/page");
    if jspage_path.exists() && jspage_path.is_dir() {
        for entry in fs::read_dir(jspage_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("js") {
                patch_file(&path, &rep)?;
            }
        }
    }

    Ok(())
}

/// Downloads RoPro source .crx package via direct HTTP and converts to ZIP buffer.
async fn download_extension_bytes() -> Result<Vec<u8>> {
    let crx_url = "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=100.0&x=id%3Dadbacgifemdbhdkfppmeilbgppmhaobf%26uc";

    let response_bytes = reqwest::get(crx_url)
        .await
        .context("Failed to download CRX package from Web Store")?
        .bytes()
        .await
        .context("Failed to read CRX response bytes")?;

    Ok(response_bytes.to_vec())
}

/// Downloads RoPro source and saves directly to a local .zip file.
async fn download_extract() -> Result<()> {
    println!("Downloading RoPro extension source...");
    let extension_bytes = download_extension_bytes().await?;

    let mut file_out = File::create("RoPro.zip").context("Failed to create RoPro.zip")?;
    file_out.write_all(&extension_bytes)?;

    println!("Downloaded RoPro source to RoPro.zip.");
    Ok(())
}

/// Downloads RoPro source, extracts, patches, and prepares directory.
async fn download_patch(selected_proxy: &str) -> Result<()> {
    println!("Downloading RoPro extension source...");
    let extension_bytes = download_extension_bytes().await?;

    let extract_dir = PathBuf::from("RoPro");
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir)?;
    }

    let mut cursor = Cursor::new(extension_bytes);
    zip_extract(&mut cursor, &extract_dir).context("Failed to extract ZIP archive")?;

    patch_extension_dir(&extract_dir, selected_proxy)?;
    println!("Finished downloading and patching.");
    Ok(())
}

/// Main entry point supporting CLI automation and interactive menu.
#[tokio::main]
async fn main() -> Result<()> {
    let proxies = get_proxies().await.unwrap_or_else(|err| {
        eprintln!("Warning: Could not fetch proxies list ({}), fallback to default.", err);
        vec!["127.0.0.1:8080".to_string()]
    });

    let args: Vec<String> = std::env::args().collect();

    // ---------------------------------------------------------
    // Unattended CLI Mode: run via `cargo run -- <proxy_idx|proxy_str>`
    // ---------------------------------------------------------
    if args.len() == 2 {
        let arg = &args[1];
        let selected_proxy = if let Ok(idx) = arg.parse::<usize>() {
            proxies
                .get(idx)
                .cloned()
                .unwrap_or_else(|| arg.to_string())
        } else {
            arg.to_string()
        };

        download_patch(&selected_proxy).await?;

        let source_dir = PathBuf::from("RoPro");
        let zip_out = PathBuf::from("RoPro-PATCHED.zip");

        zip_create_from_directory(&zip_out, &source_dir)
            .context("Unable to create output ZIP archive")?;

        fs::remove_dir_all(source_dir).context("Unable to clean up RoPro directory")?;

        println!("Automation complete! Saved output to {}", zip_out.display());
        return Ok(());
    }

    // ---------------------------------------------------------
    // Interactive TUI Menu Mode using `inquire`
    // ---------------------------------------------------------
    println!("-------------------------");
    println!("-     RoPro Patcher     -");
    println!("-------------------------");

    let options = vec![
        "Custom Patch (Local Folder / Custom Proxy)",
        "Download RoPro source as .zip",
        "Download and Patch (uses default proxy)",
        "Exit",
    ];

    let choice = Select::new("Select an action:", options).prompt()?;

    match choice {
        "Download RoPro source as .zip" => {
            download_extract().await?;
        }
        "Download and Patch (uses default proxy)" => {
            if let Some(default_proxy) = proxies.get(0) {
                download_patch(default_proxy).await?;
            } else {
                eprintln!("No proxies available to patch with.");
            }
        }
        "Custom Patch (Local Folder / Custom Proxy)" => {
            let proxy_choice = Select::new("Select a proxy:", proxies.clone()).prompt()?;

            let override_proxy = Text::new("Custom proxy (leave blank to use selected above):")
                .prompt()?;

            let selected_proxy = if override_proxy.trim().is_empty() {
                proxy_choice
            } else {
                override_proxy.trim().to_string()
            };

            let path_input = Text::new("RoPro folder path:")
                .with_default("./RoPro")
                .prompt()?;

            let target_path = PathBuf::from(path_input);
            patch_extension_dir(&target_path, &selected_proxy)?;
            println!("Finished patching folder at {:?}", target_path);
        }
        _ => println!("Goodbye!"),
    }

    Ok(())
}