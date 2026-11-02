use fdaf_aec::FdafAec;
pub const ENGINE_NAME: &str = "fdaf-aec-0.1.0(single-partition FDAF-NLMS,pure-Rust)";
const FFT: usize = 1024; // 512-sample (32 ms) blocks
struct E { a: FdafAec, rb: Vec<f32>, mb: Vec<f32>, ob: std::collections::VecDeque<f32> }
impl Engine for E {
    fn process(&mut self, r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        self.rb.extend_from_slice(r);
        self.mb.extend_from_slice(m);
        while self.rb.len() >= FFT / 2 {
            let rr: Vec<f32> = self.rb.drain(..FFT / 2).collect();
            let mm: Vec<f32> = self.mb.drain(..FFT / 2).collect();
            let o = self.a.process(&rr, &mm);
            self.ob.extend(o);
        }
        // output lags input by up to one block; fill with zeros until the first block is ready
        for _ in 0..FRAME { out.push(self.ob.pop_front().unwrap_or(0.0)); }
    }
}
pub fn make(_mode: &str) -> Box<dyn Engine> {
    let mu: f32 = std::env::var("FDAF_MU").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
    Box::new(E { a: FdafAec::new(FFT, mu), rb: vec![], mb: vec![], ob: Default::default() })
}
