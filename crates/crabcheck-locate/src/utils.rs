use {
    cpu_time::ProcessTime,
    std::time::Duration,
};

pub fn with_time<T>(f: Box<dyn FnOnce() -> T + '_>) -> (T, Duration) {
    let start = ProcessTime::now();
    let result = f();
    let duration = start.elapsed();
    (result, duration)
}
