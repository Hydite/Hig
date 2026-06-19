use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PipelineTaskClass {
    SealedHit,
    Solid,
    Batch,
    Single,
    Chunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineTask {
    pub plan_index: usize,
    pub class: PipelineTaskClass,
    pub estimated_bytes: u64,
}

#[derive(Debug, Default)]
pub struct PipelineScheduler {
    small_first: bool,
}

impl PipelineScheduler {
    pub fn new(small_first: bool) -> Self {
        Self { small_first }
    }

    pub fn order(&self, tasks: &mut [PipelineTask]) {
        if self.small_first {
            tasks.sort_by_key(|task| (task.estimated_bytes, task.class, task.plan_index));
        } else {
            tasks.sort_by_key(|task| task.plan_index);
        }
    }
}

#[derive(Debug, Clone)]
pub struct BufferPool {
    inner: Arc<Mutex<BufferPoolInner>>,
}

#[derive(Debug, Default)]
struct BufferPoolInner {
    buckets: BTreeMap<usize, Vec<Vec<u8>>>,
    hits: u64,
    misses: u64,
    peak_bytes: u64,
    live_bytes: u64,
    budget_bytes: u64,
}

#[derive(Debug)]
pub struct PooledBuffer {
    pool: BufferPool,
    bucket: usize,
    bytes: Vec<u8>,
}

impl BufferPool {
    pub fn new(budget_bytes: usize) -> Self {
        let inner = BufferPoolInner {
            budget_bytes: budget_bytes as u64,
            ..Default::default()
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn get(&self, size: usize) -> PooledBuffer {
        let bucket = bucket_size(size);
        let mut inner = self.inner.lock().expect("buffer pool lock poisoned");
        if let Some(bytes) = inner.buckets.get_mut(&bucket).and_then(Vec::pop) {
            inner.hits += 1;
            PooledBuffer {
                pool: self.clone(),
                bucket,
                bytes,
            }
        } else {
            inner.misses += 1;
            inner.live_bytes = inner.live_bytes.saturating_add(bucket as u64);
            inner.peak_bytes = inner.peak_bytes.max(inner.live_bytes);
            PooledBuffer {
                pool: self.clone(),
                bucket,
                bytes: Vec::with_capacity(bucket),
            }
        }
    }

    pub fn hits(&self) -> u64 {
        self.inner.lock().expect("buffer pool lock poisoned").hits
    }

    pub fn misses(&self) -> u64 {
        self.inner.lock().expect("buffer pool lock poisoned").misses
    }

    pub fn peak_bytes(&self) -> u64 {
        self.inner
            .lock()
            .expect("buffer pool lock poisoned")
            .peak_bytes
    }
}

impl PooledBuffer {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn bytes_mut(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        self.bytes.clear();
        let mut inner = self.pool.inner.lock().expect("buffer pool lock poisoned");
        if inner.live_bytes <= inner.budget_bytes {
            let bytes = std::mem::take(&mut self.bytes);
            inner.buckets.entry(self.bucket).or_default().push(bytes);
        } else {
            inner.live_bytes = inner.live_bytes.saturating_sub(self.bucket as u64);
        }
    }
}

fn bucket_size(size: usize) -> usize {
    const BUCKETS: [usize; 4] = [64 * 1024, 1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024];
    BUCKETS
        .into_iter()
        .find(|bucket| *bucket >= size)
        .unwrap_or_else(|| size.next_power_of_two())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_can_prioritize_small_work() {
        let scheduler = PipelineScheduler::new(true);
        let mut tasks = vec![
            PipelineTask {
                plan_index: 0,
                class: PipelineTaskClass::Chunk,
                estimated_bytes: 1024 * 1024,
            },
            PipelineTask {
                plan_index: 1,
                class: PipelineTaskClass::Batch,
                estimated_bytes: 1024,
            },
        ];
        scheduler.order(&mut tasks);
        assert_eq!(tasks[0].plan_index, 1);
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|task| task.plan_index == 0));
    }

    #[test]
    fn scheduler_preserves_plan_order_when_priority_is_disabled() {
        let scheduler = PipelineScheduler::new(false);
        let mut tasks = vec![
            PipelineTask {
                plan_index: 2,
                class: PipelineTaskClass::Chunk,
                estimated_bytes: 1,
            },
            PipelineTask {
                plan_index: 0,
                class: PipelineTaskClass::Single,
                estimated_bytes: 1024,
            },
        ];
        scheduler.order(&mut tasks);
        assert_eq!(tasks[0].plan_index, 0);
        assert_eq!(tasks[1].plan_index, 2);
    }

    #[test]
    fn buffer_pool_reuses_returned_buffers() {
        let pool = BufferPool::new(8 * 1024 * 1024);
        {
            let mut buffer = pool.get(1024);
            buffer.bytes_mut().extend_from_slice(b"abc");
            assert_eq!(pool.misses(), 1);
        }
        {
            let buffer = pool.get(1024);
            assert!(buffer.bytes.capacity() >= 64 * 1024);
            assert_eq!(pool.hits(), 1);
        }
    }
}
