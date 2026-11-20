use crate::config::Config;
use std::path::Path;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);
const LABEL: &str = "Soundscape server login";

fn attributes() -> [(&'static str, &'static str); 2] {
    [("application", crate::IDENTIFIER), ("kind", "server-token")]
}

async fn opened() -> Result<oo7::Keyring, String> {
    let keyring = oo7::Keyring::new()
        .await
        .map_err(|error| error.to_string())?;
    keyring.unlock().await.map_err(|error| error.to_string())?;
    Ok(keyring)
}

pub async fn read() -> Result<Option<String>, String> {
    tokio::time::timeout(WAIT, async {
        let keyring = opened().await?;
        let items = keyring
            .search_items(&attributes())
            .await
            .map_err(|error| error.to_string())?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        item.unlock().await.map_err(|error| error.to_string())?;
        let secret = item.secret().await.map_err(|error| error.to_string())?;
        let token = String::from_utf8(secret.to_vec()).map_err(|error| error.to_string())?;
        Ok(Some(token).filter(|token| !token.is_empty()))
    })
    .await
    .map_err(|_| "the keyring didn't answer".to_string())?
}

pub async fn write(token: Option<String>) -> Result<(), String> {
    tokio::time::timeout(WAIT, async {
        let keyring = opened().await?;
        match token {
            Some(token) => keyring
                .create_item(LABEL, &attributes(), token.as_str(), true)
                .await
                .map_err(|error| error.to_string()),
            None => keyring
                .delete(&attributes())
                .await
                .map_err(|error| error.to_string()),
        }
    })
    .await
    .map_err(|_| "the keyring didn't answer".to_string())?
}

pub fn for_disk(config: &Config, in_keyring: bool) -> Config {
    let mut copy = config.clone();
    if in_keyring {
        copy.token = None;
    }
    copy
}

pub fn save(config: &Config, dir: &Path, in_keyring: bool) {
    if let Err(error) = for_disk(config, in_keyring).save(dir) {
        eprintln!("Couldn't save settings: {error}");
    }
}

pub async fn settle(dir: &Path, mut config: Config) -> (Config, bool) {
    match read().await {
        Ok(Some(token)) => {
            let had_file_token = config.token.is_some();
            config.token = Some(token);
            if had_file_token {
                save(&config, dir, true);
            }
            (config, true)
        }
        Ok(None) => match config.token.clone() {
            Some(token) => match write(Some(token)).await {
                Ok(()) => {
                    save(&config, dir, true);
                    (config, true)
                }
                Err(error) => {
                    eprintln!("Keeping the login in the settings file: {error}");
                    (config, false)
                }
            },
            None => (config, true),
        },
        Err(error) => {
            eprintln!("No keyring, keeping the login in the settings file: {error}");
            (config, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_the_token_out_of_the_file_only_when_the_keyring_has_it() {
        let config = Config {
            server: Some("https://music.example.com/".into()),
            token: Some("secret".into()),
            ..Config::default()
        };
        assert_eq!(for_disk(&config, true).token, None);
        assert_eq!(for_disk(&config, true).server, config.server);
        assert_eq!(for_disk(&config, false).token.as_deref(), Some("secret"));
    }
}
