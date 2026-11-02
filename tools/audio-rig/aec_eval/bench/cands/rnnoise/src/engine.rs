use nnnoiseless::DenoiseState;
pub const ENGINE_NAME: &str = "nnnoiseless-0.5.2(RNNoise NS only,pure-Rust)";
// RNNoise runs at 48 kHz with 480-sample (10 ms) frames: 3x upsample, denoise, 3x downsample
// with a 63-tap windowed-sinc low-pass (cutoff 7.5 kHz). Resampler cost is included in the RTF.
const TAPS: usize = 63;
struct E { d: Box<DenoiseState<'static>>, h: Vec<f32>, up_hist: Vec<f32>, dn_hist: Vec<f32> }
fn design() -> Vec<f32> {
    let fc = 7500.0f32 / 48000.0; let m = (TAPS - 1) as f32 / 2.0;
    let mut h: Vec<f32> = (0..TAPS).map(|n| { let x = n as f32 - m;
        let s = if x == 0.0 { 2.0 * fc } else { (2.0 * std::f32::consts::PI * fc * x).sin() / (std::f32::consts::PI * x) };
        let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * n as f32 / (TAPS as f32 - 1.0)).cos(); s * w }).collect();
    let g: f32 = h.iter().sum(); for v in h.iter_mut() { *v /= g; } h
}
impl Engine for E {
    fn process(&mut self, _r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        // zero-stuff to 48 kHz (gain 3), low-pass
        let mut up = [0.0f32; 480];
        for i in 0..FRAME { up[3 * i] = m[i] * 3.0; }
        let mut x = std::mem::take(&mut self.up_hist); x.extend_from_slice(&up);
        let mut y = [0.0f32; 480];
        for n in 0..480 { let mut a = 0.0; for k in 0..TAPS { a += self.h[k] * x[n + TAPS - 1 - k]; } y[n] = a * 32768.0; }
        let keep = x.len() - (TAPS - 1); self.up_hist = x[keep..].to_vec();
        let mut o = [0.0f32; 480];
        self.d.process_frame(&mut o, &y);
        // low-pass then decimate by 3
        let mut z = std::mem::take(&mut self.dn_hist); z.extend_from_slice(&o);
        for i in 0..FRAME { let n = 3 * i; let mut a = 0.0; for k in 0..TAPS { a += self.h[k] * z[n + TAPS - 1 - k]; } out.push(a / 32768.0); }
        let keep = z.len() - (TAPS - 1); self.dn_hist = z[keep..].to_vec();
    }
}
pub fn make(_mode: &str) -> Box<dyn Engine> {
    Box::new(E { d: DenoiseState::new(), h: design(), up_hist: vec![0.0; TAPS - 1], dn_hist: vec![0.0; TAPS - 1] })
}
