use std::{fs::File, io::BufReader, path::Path, time::Duration};

use anyhow::{Context, Result};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

/// Тонкая обёртка над rodio: один трек играет в одном Sink.
pub struct Engine {
    _stream: OutputStream, // должен жить, пока играет звук
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    volume: f32,
}

impl Engine {
    pub fn new() -> Result<Self> {
        let (stream, handle) = OutputStream::try_default()
            .context("не удалось открыть устройство вывода звука по умолчанию")?;
        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            volume: 0.8,
        })
    }

    pub fn play(&mut self, path: &Path) -> Result<()> {
        let file = File::open(path)
            .with_context(|| format!("не удалось открыть файл: {}", path.display()))?;
        let source = Decoder::new(BufReader::new(file))
            .with_context(|| format!("неподдерживаемый или битый файл: {}", path.display()))?;

        if let Some(old) = self.sink.take() {
            old.stop();
        }

        let sink = Sink::try_new(&self.handle).context("не удалось создать sink")?;
        sink.set_volume(self.volume);
        sink.append(source);
        self.sink = Some(sink);
        Ok(())
    }

    pub fn pause(&self) {
        if let Some(s) = &self.sink {
            s.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(s) = &self.sink {
            s.play();
        }
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
    }

    pub fn is_finished(&self) -> bool {
        self.sink.as_ref().map_or(true, |s| s.empty())
    }

    pub fn position(&self) -> Duration {
        self.sink.as_ref().map(|s| s.get_pos()).unwrap_or_default()
    }

    pub fn seek(&self, pos: Duration) {
        if let Some(s) = &self.sink {
            let _ = s.try_seek(pos);
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(s) = &self.sink {
            s.set_volume(self.volume);
        }
    }
}
