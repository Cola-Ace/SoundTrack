use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

use crate::model::{Route, RouteControls};

const SAMPLE_RATE: u64 = 48_000;
const CHANNELS: usize = 2;
const BLOCK_FRAMES: u64 = 480;
const BLOCK_SAMPLES: usize = BLOCK_FRAMES as usize * CHANNELS;
const HORIZON_BLOCKS: u64 = 2;
const MAX_QUEUED_BLOCKS: usize = 100;
const QPC_TICKS_PER_SECOND: u64 = 10_000_000;

#[derive(Clone)]
pub struct AudioEngine {
  sender: Sender<AudioPacket>,
  drop_receiver: Receiver<AudioPacket>,
  receiver: Receiver<AudioPacket>,
  controls: Arc<parking_lot::RwLock<BTreeMap<Route, RouteControls>>>,
  levels: Arc<parking_lot::RwLock<BTreeMap<Route, f32>>>,
  output: Arc<parking_lot::Mutex<BTreeMap<Route, VecDeque<MixedAudioFrame>>>>,
}

#[derive(Clone)]
pub struct AudioPacket {
  pub route: Route,
  pub qpc_time_100ns: u64,
  pub samples: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct MixedAudioFrame {
  pub sequence: u64,
  pub qpc_time_100ns: u64,
  pub frames: u32,
  pub samples: Vec<f32>,
}

impl AudioEngine {
  pub fn new(controls: BTreeMap<Route, RouteControls>) -> Self {
    let (sender, receiver) = bounded(128);
    Self {
      sender,
      drop_receiver: receiver.clone(),
      receiver,
      controls: Arc::new(parking_lot::RwLock::new(controls)),
      levels: Arc::new(parking_lot::RwLock::new(
        Route::ALL.into_iter().map(|route| (route, 0.0)).collect(),
      )),
      output: Arc::new(parking_lot::Mutex::new(
        Route::ALL
          .into_iter()
          .map(|route| (route, VecDeque::new()))
          .collect(),
      )),
    }
  }

  pub fn start(&self) {
    let receiver = self.receiver.clone();
    let controls = self.controls.clone();
    let levels = self.levels.clone();
    let output = self.output.clone();
    thread::spawn(move || mix_loop(receiver, controls, levels, output));
  }

  pub fn set_controls(&self, controls: BTreeMap<Route, RouteControls>) {
    *self.controls.write() = controls;
  }

  pub fn send(&self, packet: AudioPacket) {
    match self.sender.try_send(packet) {
      Ok(()) => {}
      Err(TrySendError::Full(packet)) => {
        let _ = self.drop_receiver.try_recv();
        let _ = self.sender.try_send(packet);
      }
      Err(TrySendError::Disconnected(_)) => {}
    }
  }

  pub fn level(&self, route: Route) -> f32 {
    self.levels.read().get(&route).copied().unwrap_or_default()
  }

  pub fn take_route_audio(&self, route: Route) -> Vec<MixedAudioFrame> {
    self
      .output
      .lock()
      .get_mut(&route)
      .map(|frames| frames.drain(..).collect())
      .unwrap_or_default()
  }
}

fn mix_loop(
  receiver: Receiver<AudioPacket>,
  controls: Arc<parking_lot::RwLock<BTreeMap<Route, RouteControls>>>,
  levels: Arc<parking_lot::RwLock<BTreeMap<Route, f32>>>,
  output: Arc<parking_lot::Mutex<BTreeMap<Route, VecDeque<MixedAudioFrame>>>>,
) {
  let mut pending = HashMap::<(Route, u64), Vec<f32>>::new();
  let mut latest_block = None::<u64>;
  let mut next_block = None::<u64>;
  let mut last_levels = Route::ALL
    .into_iter()
    .map(|route| (route, 0.0))
    .collect::<BTreeMap<_, _>>();
  let mut smoothed_gains = Route::ALL
    .into_iter()
    .map(|route| {
      let gain = controls
        .read()
        .get(&route)
        .cloned()
        .unwrap_or_default()
        .gain;
      (route, gain)
    })
    .collect::<BTreeMap<_, _>>();

  loop {
    let Ok(packet) = receiver.recv_timeout(Duration::from_millis(20)) else {
      for level in last_levels.values_mut() {
        *level *= 0.72;
      }
      *levels.write() = last_levels.clone();
      continue;
    };
    if packet.samples.len() < CHANNELS || packet.qpc_time_100ns == 0 {
      continue;
    }

    let first_sample = (u128::from(packet.qpc_time_100ns) * u128::from(SAMPLE_RATE)
      / u128::from(QPC_TICKS_PER_SECOND)) as u64;
    let first_block = first_sample / BLOCK_FRAMES;
    next_block.get_or_insert(first_block);

    for (frame_index, stereo) in packet.samples.chunks_exact(CHANNELS).enumerate() {
      let absolute_sample = first_sample.saturating_add(frame_index as u64);
      let block = absolute_sample / BLOCK_FRAMES;
      if next_block.is_some_and(|next| block < next) {
        continue;
      }
      let offset = (absolute_sample % BLOCK_FRAMES) as usize * CHANNELS;
      let samples = pending
        .entry((packet.route, block))
        .or_insert_with(|| vec![0.0; BLOCK_SAMPLES]);
      for channel in 0..CHANNELS {
        let sample = stereo[channel];
        if sample.is_finite() {
          samples[offset + channel] += sample;
        }
      }
      latest_block = Some(latest_block.map_or(block, |latest| latest.max(block)));
    }

    let Some(latest) = latest_block else {
      continue;
    };
    let flush_through = latest.saturating_sub(HORIZON_BLOCKS);
    while next_block.is_some_and(|next| next <= flush_through) {
      let block = next_block.expect("next block is set");
      let block_time = block
        .saturating_mul(BLOCK_FRAMES)
        .saturating_mul(QPC_TICKS_PER_SECOND)
        / SAMPLE_RATE;
      for route in Route::ALL {
        let mut samples = pending
          .remove(&(route, block))
          .unwrap_or_else(|| vec![0.0; BLOCK_SAMPLES]);
        let route_controls = controls.read().get(&route).cloned().unwrap_or_default();
        let target_gain = if route_controls.muted {
          0.0
        } else {
          route_controls.gain
        };
        let previous_gain = smoothed_gains.get(&route).copied().unwrap_or(target_gain);
        let gain = previous_gain + (target_gain - previous_gain) * 0.5;
        smoothed_gains.insert(route, gain);
        let mut peak = 0.0_f32;
        for sample in &mut samples {
          *sample *= gain;
          peak = peak.max(sample.abs());
          *sample = sample.clamp(-1.0, 1.0);
        }
        last_levels.insert(route, peak);
        let frame = MixedAudioFrame {
          sequence: block,
          qpc_time_100ns: block_time,
          frames: BLOCK_FRAMES as u32,
          samples,
        };
        let mut output = output.lock();
        let queue = output.entry(route).or_default();
        if queue.len() == MAX_QUEUED_BLOCKS {
          queue.pop_front();
        }
        queue.push_back(frame);
      }
      next_block = Some(block.saturating_add(1));
    }

    pending.retain(|(_, block), _| next_block.is_some_and(|next| *block >= next));
    *levels.write() = last_levels.clone();
  }
}
