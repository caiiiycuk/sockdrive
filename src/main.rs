use ctrlc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod layer;
use layer::{Layer, SECTOR_SIZE};

#[tokio::main(worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    struct ReadRequest {
        sector: u32,
        ahead: u8,
        sender: mpsc::Sender<SectorResponse>,
    }

    struct WriteRequest {
        sector: u32,
        bytes: [u8; SECTOR_SIZE],
    }

    struct SectorResponse {
        bytes: Vec<u8>,
    }

    let live = Arc::new(AtomicBool::new(true));
    let live_handler = live.clone();
    ctrlc::set_handler(move || {
        live_handler.store(false, Ordering::Relaxed);
        println!("Exiting, please wait...");
    })
    .expect("Error setting Ctrl-C handler");

    let mut layer = Layer::new("drive-0", 2048 / SECTOR_SIZE * 1024 * 1024);
    let listener = TcpListener::bind("0.0.0.0:8002").await?;

    println!("sockdrive is started");

    let (read_tx, mut read_rx) = mpsc::channel::<ReadRequest>(128);
    let (write_tx, mut write_rx) = mpsc::channel::<WriteRequest>(128);

    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                let read_tx = read_tx.clone();
                let write_tx = write_tx.clone();
                let (sector_tx, mut sector_rx) = mpsc::channel::<SectorResponse>(1);
                tokio::spawn(async move {
                    loop {
                        let mut sector_buf: [u8; SECTOR_SIZE] = [0; SECTOR_SIZE];
                        match (stream.read_u8().await, stream.read_u32_le().await) {
                            (Ok(1), Ok(sector)) => {
                                let ahead = stream.read_u8().await;
                                if ahead.is_err() ||  read_tx
                                    .send(ReadRequest {
                                        sector,
                                        ahead: ahead.unwrap(),
                                        sender: sector_tx.clone(),
                                    })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }

                                if let Some(response) = sector_rx.recv().await {
                                    if stream.write_all(&response.bytes).await.is_err() {
                                        return;
                                    }
                                } else {
                                    return;
                                }
                            }
                            (Ok(2), Ok(sector)) => {
                                if stream.read_exact(&mut sector_buf).await.is_err() {
                                    return;
                                }

                                if write_tx
                                    .send(WriteRequest {
                                        sector,
                                        bytes: sector_buf.clone(),
                                    })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            _ => {
                                return;
                            }
                        }
                    }
                });
            }
        }
    });

    let mut prev_total_reads = 0;
    let mut prev_total_writes = 0;
    let mut total_reads = 0;
    let mut total_writes = 0;
    let mut sleep_times = 0;
    let mut reported_reads = 0;
    let mut reported_writes = 0;
    let mut reported_sleep_times = 0;
    let mut read_buffer: [u8; SECTOR_SIZE] = [0; SECTOR_SIZE];

    while live.load(Ordering::Relaxed) {
        loop {
            match write_rx.try_recv() {
                Ok(request) => {
                    total_writes += 1;
                    layer.write(request.sector, &request.bytes);
                }
                _ => {
                    break;
                }
            }
        }

        loop {
            match read_rx.try_recv() {
                Ok(request) => {
                    total_reads += 1;
                    let mut bytes: Vec<u8> = Vec::new();
                    for i in 0..request.ahead {
                        // TODO maybe is better to allocate vector and not use extend
                        layer.read(request.sector + i as u32, &mut read_buffer);
                        bytes.extend(read_buffer);
                    }
                    debug_assert!(bytes.len() == SECTOR_SIZE * request.ahead as usize);
                    let _ = request
                        .sender
                        .send(SectorResponse {
                            bytes,
                        })
                        .await;
                }
                _ => {
                    break;
                }
            }
        }

        if total_reads - reported_reads > 1000
            || total_writes - reported_writes > 1000
            || sleep_times - reported_sleep_times > 1000
        {
            println!(
            "Reads {}, Writes {}, Sleeps {}",
            total_reads, total_writes, sleep_times
            );
            reported_reads = total_reads;
            reported_writes = total_writes;
            reported_sleep_times = sleep_times;
        }

        if prev_total_reads == total_reads && prev_total_writes == total_writes {
            std::thread::sleep(std::time::Duration::from_millis(1));
            sleep_times += 1;
        }

        prev_total_reads = total_reads;
        prev_total_writes = total_writes;
    }

    layer.flush();
    println!("drive is flushed, sockdrive is stopping...");
    
    Ok(())
}
