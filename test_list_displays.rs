//! Test program to list displays using lunaris-media.
//! Run: cargo run --example test_list_displays

use lunaris_media::capture::create_screen_capture;

#[tokio::main]
async fn main() {
    env_logger::init();

    println!("Creating screen capture backend...");
    let cap = match create_screen_capture() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to create screen capture: {:?}", e);
            return;
        }
    };

    println!("Listing displays...");
    match cap.list_displays().await {
        Ok(displays) => {
            println!("Found {} display(s):", displays.len());
            for (i, d) in displays.iter().enumerate() {
                println!(
                    "  [{}] id={:?} name={:?} {}x{} @{:.1}Hz primary={}",
                    i, d.id, d.name, d.width, d.height, d.refresh_rate, d.is_primary
                );
            }
        }
        Err(e) => {
            eprintln!("Failed to list displays: {:?}", e);
        }
    }
}
