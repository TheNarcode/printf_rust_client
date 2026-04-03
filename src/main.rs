use std::fs::{self, File};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{Config, PrintAttributes};
use eventsource_client::{self as es};
use eventsource_client::{Client, SSE};
use ftail::Ftail;
use futures::TryStreamExt;
use log::LevelFilter;

pub mod ipp;
pub mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let logs_dir = dirs::data_local_dir().unwrap().join("printf").join("logs");
    fs::create_dir_all(&logs_dir)?;

    Ftail::new()
        .daily_file(&logs_dir, LevelFilter::Info)
        .init()?;

    log::info!("printf client started");

    let config = read_config()?;

    let client = es::ClientBuilder::for_url(&config.event_url)?
        .reconnect(
            es::ReconnectOptions::reconnect(true)
                .retry_initial(false)
                .delay(Duration::from_secs(1))
                .backoff_factor(2)
                .delay_max(Duration::from_secs(60))
                .build(),
        )
        .build();

    let printers = get_ipp_printers().await?;
    let pm = Arc::new(Mutex::new(PrinterManager::new(printers)));

    log::info!("printer manager initialized");

    client
        .stream()
        .try_for_each(|event| async {
            let pm = pm.clone();

            if let SSE::Event(e) = event {
                if let "update" = e.event_type.as_str() {
                    log::info!("got new print command");

                    let attributes: PrintAttributes = match serde_json::from_str(&e.data) {
                        Ok(attr) => attr,
                        Err(err) => {
                            log::error!("failed to parse print attributes: {}", err);
                            return Ok(());
                        }
                    };

                    let printer = {
                        let mut pm_guard = pm.lock().unwrap();
                        match pm_guard.get_printer(&attributes.color) {
                            Some(p) => p,
                            None => {
                                log::error!(
                                    "no printer found for color mode: {:?}",
                                    attributes.color
                                );
                                return Ok(());
                            }
                        }
                    };

                    tokio::spawn(async move {
                        log::info!("using printer {} for print", printer.uri);

                        match printer.uri.parse() {
                            Ok(uri) => match print_job(uri, attributes).await {
                                Ok(_) => log::info!("print job successful"),
                                Err(e) => log::error!("print job failed: {}", e),
                            },
                            Err(e) => log::error!("failed to parse printer URI: {}", e),
                        }
                    });
                }
            }

            Ok(())
        })
        .await?;

    Ok(())
}

pub fn read_config() -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
    let config_dir = dirs::config_local_dir()
        .unwrap()
        .join("printf")
        .join("config.json");

    let file = File::open(&config_dir)?;
    Ok(serde_json::from_reader(file)?)
}
