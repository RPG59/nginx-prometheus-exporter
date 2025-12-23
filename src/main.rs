use axum::middleware;
use axum::{http::HeaderValue, http::StatusCode, response::Response, routing::get, Router};
use clap::Parser;
use glob::glob;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/*
TODO:
- logger
- version command
 */

#[derive(Parser, Debug)]
#[command(author, version, about = "Nginx Prometheus Exporter", long_about = None)]
struct Args {
    #[arg(short, long, default_value = "./docker/logs/*.log")]
    log_path: String,

    #[arg(short, long, default_value = "6969")]
    port: u16,
}

#[derive(Debug, Deserialize)]
struct NginxLogEntry {
    http: HttpData,
    nginx: NginxData,
}

#[derive(Debug, Deserialize)]
struct HttpData {
    response: ResponseData,
}

#[derive(Debug, Deserialize)]
struct ResponseData {
    status_code: String,
}

#[derive(Debug, Deserialize)]
struct NginxData {
    access: AccessData,
    time: TimeData,
}

#[derive(Debug, Deserialize)]
struct AccessData {
    method: String,
    url: String,
    host: String,
}

#[derive(Debug, Deserialize)]
struct TimeData {
    request: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct MetricLabels {
    method: String,
    path: String,
    status_code: String,
    host: String,
}

fn get_status_label(status_code: String) -> Result<&'static str, String> {
    let status = status_code
        .parse::<u16>()
        .map_err(|e| format!("Failed to parse status_code. Error: {}", e))?;

    match status {
        100..=199 => Ok("1xx"),
        200..=299 => Ok("2xx"),
        300..=399 => Ok("3xx"),
        400..=499 => Ok("4xx"),
        500..=599 => Ok("5xx"),
        _ => Err("Unknown status code".to_string()),
    }
}

struct LogFileMeta {
    file_position: u64,
    inode: u64,
}

struct MetricsState {
    log_files: HashMap<PathBuf, LogFileMeta>,
    metrics: HashMap<MetricLabels, Vec<f64>>,
    pattern: String,
}

impl MetricsState {
    fn new(pattern: String) -> Self {
        Self {
            log_files: HashMap::new(),
            metrics: HashMap::new(),
            pattern,
        }
    }

    fn update_files_map(&mut self) {
        for entry in glob(&self.pattern).expect("Failed to read glob pattern") {
            match entry {
                Ok(path) => {
                    if let Some(_) = self.log_files.get(&path) {
                        continue;
                    }

                    let inode = std::fs::metadata(&path).unwrap().ino();

                    self.log_files.insert(
                        path,
                        LogFileMeta {
                            file_position: 0,
                            inode,
                        },
                    );
                }
                Err(e) => println!("{:?}", e),
            }
        }
    }

    fn check_file_rotation(path: &PathBuf, meta: &LogFileMeta) -> Result<bool, String> {
        let metadata =
            std::fs::metadata(path).map_err(|e| format!("Failed to get file metadata: {}", e))?;

        if meta.inode != metadata.ino() || meta.file_position > metadata.len() {
            return Ok(true);
        }

        Ok(false)
    }

    fn read_new_entries(&mut self) -> Result<HashMap<MetricLabels, Vec<f64>>, String> {
        for (path, meta) in &mut self.log_files {
            if MetricsState::check_file_rotation(path, meta)? {
                meta.file_position = 0;
            }

            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .map_err(|e| format!("Failed to open log file: {}", e))?;

            let mut reader = BufReader::new(file);

            reader
                .seek(SeekFrom::Start(meta.file_position))
                .map_err(|e| format!("Failed to seek to position: {}", e))?;

            let mut line = String::new();

            loop {
                let bytes_read = reader
                    .read_line(&mut line)
                    .map_err(|e| format!("Failed to read line: {}", e))?;

                if bytes_read == 0 {
                    break;
                }

                if !line.trim().is_empty() {
                    match serde_json::from_str::<NginxLogEntry>(&line) {
                        Ok(entry) => {
                            if let Ok(duration) = entry.nginx.time.request.parse::<f64>() {
                                let labels = MetricLabels {
                                    method: entry.nginx.access.method,
                                    path: entry.nginx.access.url,
                                    status_code: get_status_label(entry.http.response.status_code)?
                                        .to_string(),
                                    host: entry.nginx.access.host,
                                };
                                self.metrics
                                    .entry(labels)
                                    .or_insert_with(Vec::new)
                                    .push(duration);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to parse log line: {} - Error: {}", line.trim(), e);
                        }
                    }
                }

                meta.file_position += bytes_read as u64;
                line.clear();
            }
        }

        Ok(self.metrics.clone())
    }
}

fn calculate_quantile(sorted_data: &[f64], quantile: f64) -> f64 {
    if sorted_data.is_empty() {
        return 0.0;
    }

    let index = (quantile * (sorted_data.len() - 1) as f64).round() as usize;
    sorted_data[index]
}

fn exponential_buckets(start: f64, factor: f64, count: usize) -> Vec<f64> {
    let mut buckets = Vec::with_capacity(count);
    let mut current = start;

    for _ in 0..count {
        buckets.push(current);
        current *= factor;
    }

    buckets
}

fn calculate_histogram_buckets(data: &[f64], buckets: &[f64]) -> Vec<usize> {
    let mut counts = vec![0; buckets.len()];

    for &value in data {
        for (i, &bucket) in buckets.iter().enumerate() {
            if value <= bucket {
                counts[i] += 1;
            }
        }
    }

    counts
}

async fn metrics_handler(state: Arc<Mutex<MetricsState>>) -> (StatusCode, String) {
    let mut state = state.lock().unwrap();

    state.update_files_map();

    let buckets = exponential_buckets(0.05, 2.0, 10);

    let mut output: Vec<String> = vec![
        "# HELP nginx_http_request_duration_seconds Request duration in seconds".to_string(),
        "# TYPE nginx_http_request_duration_seconds histogram".to_string(),
    ];

    let metrics_map = match state.read_new_entries() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error reading log entries: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("# Error: {}\n", e),
            );
        }
    };

    for (labels, durations) in metrics_map.iter() {
        let sum: f64 = durations.iter().sum();
        let count = durations.len();
        let mut sorted_durations = durations.clone();

        sorted_durations.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Calculate histogram buckets
        let bucket_counts = calculate_histogram_buckets(durations, &buckets);

        let label_str = format!(
            "method=\"{}\",path=\"{}\",status_code=\"{}\",host=\"{}\"",
            labels.method, labels.path, labels.status_code, labels.host
        );

        // Output histogram buckets
        for (i, &bucket_limit) in buckets.iter().enumerate() {
            output.push(format!(
                "nginx_http_request_duration_seconds_bucket{{{},le=\"{}\"}} {}",
                label_str, bucket_limit, bucket_counts[i]
            ));
        }

        // Add +Inf bucket (all values)
        output.push(format!(
            "nginx_http_request_duration_seconds_bucket{{{},le=\"+Inf\"}} {}",
            label_str, count
        ));

        // Output sum and count
        output.push(format!(
            "nginx_http_request_duration_seconds_sum{{{}}} {}",
            label_str, sum
        ));

        output.push(format!(
            "nginx_http_request_duration_seconds_count{{{}}} {}",
            label_str, count
        ));
    }

    (StatusCode::OK, output.join("\n"))
}

async fn custom_header_middleware<B>(mut response: Response<B>) -> Response<B> {
    response.headers_mut().insert(
        "X-Powered-By",
        HeaderValue::from_static("nginx-prometheus-exporter"),
    );
    response
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    println!("Starting Nginx Prometheus Exporter");
    println!("Log file: {:?}", args.log_path);
    println!("Server port: {}", args.port);

    let state = Arc::new(Mutex::new(MetricsState::new(args.log_path)));

    let app = Router::new()
        .route(
            "/metrics",
            get({
                let state = Arc::clone(&state);
                move || metrics_handler(state)
            }),
        )
        .layer(middleware::map_response(custom_header_middleware));

    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    println!("Server listening on {}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server failed to start");
}
