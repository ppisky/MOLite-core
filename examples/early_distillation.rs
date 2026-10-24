use molite_core::{Scheduler, Turn};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut scheduler = Scheduler::open(":memory:")?;
    scheduler.configure("demo", 12)?;
    for n in 1..=5 {
        scheduler.record(
            "demo",
            &Turn {
                request_id: format!("turn-{n}"),
                user: format!("用户输入 {n}"),
                assistant: format!("已完成回答 {n}"),
            },
        )?;
    }
    println!(
        "提前前: {}",
        serde_json::to_string(&scheduler.status("demo")?)?
    );
    let batch = scheduler.begin("demo", true)?.expect("pending evidence");
    // Demonstration only: an actual worker must call the model and validate its
    // DMW patch before acknowledging. An empty patch is a legitimate no-op.
    scheduler.complete(&batch, &json!({"patches": []}))?;
    println!(
        "提前成功后: {}",
        serde_json::to_string(&scheduler.status("demo")?)?
    );
    Ok(())
}
