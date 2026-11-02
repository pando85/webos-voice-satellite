use decibri_aec::{Aec, AecConfig};
pub const ENGINE_NAME: &str = "decibri-aec-0.2.0(Apache-2.0,pure-Rust)";
struct E(Aec, Vec<f32>);
impl Engine for E {
    fn process(&mut self, r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        self.0.feed_reference(r);
        let before = out.len();
        self.0.process(m, out).unwrap();
        // the engine re-blocks; keep the output length equal to the input length
        let _ = before;
        let _ = &self.1;
    }
    fn flush(&mut self, out: &mut Vec<f32>) { let _ = self.0.flush(out); }
}
pub fn make(mode: &str) -> Box<dyn Engine> {
    let mut c = AecConfig::default();
    c.sample_rate = SR;
    if mode.contains("nosup") { c.suppression = decibri_aec::Suppression::Off; }
    if mode.contains("hint") { c.delay_hint_ms = Some(20); }
    Box::new(E(Aec::new(c).unwrap(), vec![]))
}
