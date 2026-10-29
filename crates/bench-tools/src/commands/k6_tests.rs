use super::*;
use std::path::Path;

#[test]
fn reports_warn_when_the_generator_is_the_bottleneck() -> Result<()> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/k6/constant");
    let mut run = Run::load(&fixture)?;
    run.samples.clear();
    run.meta.pinned = true;
    run.meta.k6_cpu_cores = Some(1.0);
    let baseline = Run::load(&fixture)?;
    for style in [Style::Plain, Style::Markdown, Style::Color] {
        let text = report(&run, style);
        assert!(text.contains("k6 was the bottleneck"));
        assert!(delta(&baseline, &run, style).contains("candidate was generator-saturated"));
        assert!(delta(&run, &baseline, style).contains("baseline was generator-saturated"));
        if style == Style::Color {
            assert!(text.contains("\x1b[31m"));
        }
    }
    Ok(())
}
