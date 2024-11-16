use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dirs::home_dir;
use ignore::WalkBuilder;
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use serde_json::Value; // Add this back
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    domain: Option<String>,
    apikey: Option<String>,
    vault_path: Option<Vec<String>>,
    proxy_names: Option<Vec<String>>,
    #[serde(default = "default_api_url")]
    api_url: String,
}

#[derive(Debug)]
struct FileEntry {
    relative_path: PathBuf,
    absolute_path: PathBuf,
    file_type: FileType,
}

#[derive(Debug, PartialEq)]
enum FileType {
    Markdown,
    Image,
}

fn default_api_url() -> String {
    "http://localhost:5173".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            domain: None,
            apikey: None,
            vault_path: Some(vec![]),
            proxy_names: Some(vec![]),
            api_url: default_api_url(),
        }
    }
}

impl Config {
    fn new() -> Self {
        Self::default()
    }

    fn get_config_path() -> Result<PathBuf> {
        let home = home_dir().context("Could not determine home directory")?;
        Ok(home.join(".config/markopolis/markopolis.yaml"))
    }

    fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let yaml = serde_yaml::to_string(&self)?;
        fs::write(&config_path, yaml)?;
        println!("Configuration saved to: {}", config_path.display());
        Ok(())
    }

    fn read() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        if !config_path.exists() {
            anyhow::bail!("Configuration file not found. Please run 'md configure' first.");
        }
        let contents = fs::read_to_string(config_path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    async fn upload_markdown(&self, file_info: &FileEntry) -> Result<()> {
        let file = fs::read(&file_info.absolute_path)?;
        let form = multipart::Form::new()
            .text("url", file_info.relative_path.to_string_lossy().to_string())
            .part(
                "file",
                multipart::Part::bytes(file)
                    .file_name(
                        file_info
                            .absolute_path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string(),
                    )
                    .mime_str("text/markdown")?,
            );

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/api/files", self.api_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.apikey.as_deref().unwrap_or("")),
            )
            .multipart(form)
            .send()
            .await?;

        if response.status().is_success() {
            println!(
                "Successfully uploaded markdown: {}",
                file_info.relative_path.display()
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to upload markdown: {}",
                file_info.relative_path.display()
            ))
        }
    }

    async fn upload_image(&self, file_info: &FileEntry) -> Result<()> {
        let file = fs::read(&file_info.absolute_path)?;
        let mime_type = match file_info
            .absolute_path
            .extension()
            .and_then(|ext| ext.to_str())
        {
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("png") => "image/png",
            Some("svg") => "image/svg+xml",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "application/octet-stream",
        };

        let form = multipart::Form::new()
            .text("url", file_info.relative_path.to_string_lossy().to_string())
            .part(
                "file",
                multipart::Part::bytes(file)
                    .file_name(
                        file_info
                            .absolute_path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string(),
                    )
                    .mime_str(mime_type)?,
            );

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/api/files/images", self.api_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.apikey.as_deref().unwrap_or("")),
            )
            .multipart(form)
            .send()
            .await?;

        if response.status().is_success() {
            println!(
                "Successfully uploaded image: {}",
                file_info.relative_path.display()
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to upload image: {}",
                file_info.relative_path.display()
            ))
        }
    }

    async fn delete_remote_file(&self, file_id: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .delete(format!("{}/api/files/{}", self.api_url, file_id))
            .header(
                "Authorization",
                format!("Bearer {}", self.apikey.as_deref().unwrap_or("")),
            )
            .send()
            .await?;

        if response.status().is_success() {
            println!("Successfully deleted remote file with ID: {}", file_id);
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to delete remote file with ID: {}",
                file_id
            ))
        }
    }

    async fn update_wikilinks(&self) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/api/wikilinks", self.api_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.apikey.as_deref().unwrap_or("")),
            )
            .send()
            .await?;

        if response.status().is_success() {
            #[derive(Debug, Deserialize)]
            struct WikiLinksResponse {
                success: bool,
                summary: WikiLinksSummary,
            }

            #[derive(Debug, Deserialize)]
            struct WikiLinksSummary {
                #[serde(rename = "totalProcessed")]
                total_processed: i32,
                #[serde(rename = "totalAdded")]
                total_added: i32,
                #[serde(rename = "totalRemoved")]
                total_removed: i32,
                #[serde(rename = "totalInvalid")]
                total_invalid: i32,
            }

            let result = response
                .json::<WikiLinksResponse>()
                .await
                .context("Failed to parse wikilinks response")?;

            println!("\nWikiLinks Update Summary:");
            println!(
                "- Total files processed: {}",
                result.summary.total_processed
            );
            println!("- New links added: {}", result.summary.total_added);
            println!("- Old links removed: {}", result.summary.total_removed);
            println!("- Invalid links found: {}", result.summary.total_invalid);
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(anyhow::anyhow!(
                "Failed to update wikilinks. Server response: {}",
                error_text
            ))
        }
    }

    async fn trigger_todo_extraction(&self) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/api/todo", self.api_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.apikey.as_deref().unwrap_or("")),
            )
            .send()
            .await?;

        if response.status().is_success() {
            println!("Todo extraction and processing triggered successfully.");
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(anyhow::anyhow!(
                "Failed to trigger todo extraction. Server response: {}",
                error_text
            ))
        }
    }

    async fn sync(&self, dry_run: bool) -> Result<()> {
        let local_files = self.collect_files()?;
        let remote_files = self.get_remote_files().await?;

        let local_file_paths: HashSet<String> = local_files
            .values()
            .flat_map(|files| {
                files
                    .iter()
                    .map(|file| file.relative_path.to_string_lossy().to_string())
            })
            .collect();

        let remote_file_paths: HashSet<_> = remote_files.keys().cloned().collect();

        let files_to_delete: HashSet<_> = remote_file_paths.difference(&local_file_paths).collect();

        if dry_run {
            println!("Dry run - files that would be deleted:");
            for file in &files_to_delete {
                println!("{}", file);
            }
            return Ok(());
        }

        // Delete files that exist remotely but not locally
        for file in files_to_delete {
            if let Some(file_id) = remote_files.get(file) {
                self.delete_remote_file(file_id).await?;
            }
        }

        // Upload or update local files
        for (_proxy_path, files) in local_files {
            for file in files {
                if file.file_type == FileType::Markdown {
                    self.upload_markdown(&file).await?;
                } else if file.file_type == FileType::Image {
                    self.upload_image(&file).await?;
                }
            }
        }

        println!("File sync completed successfully.");

        // Update wikilinks after all files have been uploaded
        println!("\nUpdating wikilinks...");
        match self.update_wikilinks().await {
            Ok(_) => println!("Wikilinks updated successfully."),
            Err(e) => eprintln!("Error updating wikilinks: {:?}", e),
        }

        // Trigger todo extraction after wikilinks update
        println!("\nTriggering todo extraction...");
        match self.trigger_todo_extraction().await {
            Ok(_) => println!("Todo extraction completed successfully."),
            Err(e) => eprintln!("Error triggering todo extraction: {:?}", e),
        }

        println!("\nSync process completed successfully.");
        Ok(())
    }

    async fn get_remote_files(&self) -> Result<HashMap<String, String>> {
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{}/api/files", self.api_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.apikey.as_deref().unwrap_or("")),
            )
            .send()
            .await?;

        let body = response.text().await?;

        #[derive(Deserialize)]
        struct FileResponse {
            files: Vec<FileInfo>,
        }

        #[derive(Deserialize)]
        struct FileInfo {
            id: String,
            mdpath: String,
        }

        let response: FileResponse =
            serde_json::from_str(&body).context("Failed to parse JSON response from server")?;

        // Create HashMap with mdpath as the key and id as the value
        // This allows us to compare directly with the relative_path from FileEntry
        let mut files_map = HashMap::new();
        for file in response.files {
            files_map.insert(file.mdpath, file.id);
        }

        Ok(files_map)
    }

    fn collect_files(&self) -> Result<HashMap<PathBuf, Vec<FileEntry>>> {
        let vault_paths = self
            .vault_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Vault paths not configured"))?;

        let files_by_proxy: Arc<Mutex<HashMap<PathBuf, Vec<FileEntry>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        for vault_path in vault_paths {
            let vault_pathbuf = PathBuf::from(vault_path);

            let file_walker = WalkBuilder::new(&vault_pathbuf)
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .threads(num_cpus::get())
                .build_parallel();

            file_walker.run(|| {
                let files_by_proxy = Arc::clone(&files_by_proxy);

                Box::new(move |result| {
                    if let Ok(entry) = result {
                        let path = entry.path();

                        // Check if the current directory is a "notes" folder
                        if path.is_dir() && path.file_name() == Some("notes".as_ref()) {
                            let notes_folder = path.to_path_buf();
                            let mut collected_files = Vec::new();

                            // Check for settings.yaml and get mdpath if available
                            let mdpath = {
                                let settings_file = notes_folder.join("settings.yaml");
                                if settings_file.exists() {
                                    if let Ok(settings_content) = fs::read_to_string(&settings_file)
                                    {
                                        if let Ok(settings) =
                                            serde_yaml::from_str::<HashMap<String, Value>>(
                                                &settings_content,
                                            )
                                        {
                                            settings
                                                .get("mdpath")
                                                .and_then(|v| v.as_str())
                                                .map(String::from)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            };

                            // Use mdpath as a virtual base path, or default to the notes_folder if mdpath is not specified
                            let virtual_base_path =
                                PathBuf::from(mdpath.unwrap_or_else(|| "".to_string()));

                            // Collect files within the "notes" folder
                            for file in WalkBuilder::new(&notes_folder).hidden(false).build() {
                                if let Ok(file_entry) = file {
                                    if let Some(file_type) = get_file_type(file_entry.path()) {
                                        // Construct the relative path from virtual_base_path, assuming it as a logical path
                                        let mut relative_path = file_entry
                                            .path()
                                            .strip_prefix(&notes_folder)
                                            .unwrap()
                                            .to_path_buf()
                                            .components()
                                            .fold(virtual_base_path.clone(), |mut acc, comp| {
                                                acc.push(comp);
                                                acc
                                            });

                                        // Remove any leading `./` if present
                                        if let Some(stripped) = relative_path
                                            .to_str()
                                            .map(|s| s.trim_start_matches("./"))
                                        {
                                            relative_path = PathBuf::from(stripped);
                                        }

                                        collected_files.push(FileEntry {
                                            relative_path,
                                            absolute_path: file_entry.path().to_path_buf(),
                                            file_type,
                                        });
                                    }
                                }
                            }

                            if !collected_files.is_empty() {
                                files_by_proxy
                                    .lock()
                                    .unwrap()
                                    .insert(notes_folder, collected_files);
                            }
                        }
                    }
                    ignore::WalkState::Continue
                })
            });
        }

        Ok(Arc::try_unwrap(files_by_proxy)
            .unwrap()
            .into_inner()
            .unwrap())
    }
}

fn get_file_type(path: &Path) -> Option<FileType> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_lowercase())
        .and_then(|ext| match ext.as_str() {
            "md" | "markdown" => Some(FileType::Markdown),
            "jpg" | "jpeg" | "png" | "svg" | "webp" => Some(FileType::Image),
            _ => None,
        })
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Configure,
    Sync {
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Configure => {
            println!("Starting configuration...");
            let config = Config::new();
            config.save()?;
            println!("Configuration completed successfully!");
        }
        Commands::Sync { dry_run } => {
            println!("Starting sync...");
            let config = Config::read()?;
            config.sync(*dry_run).await?;
        }
    }

    Ok(())
}
