use axum::{http::StatusCode, routing::get, Router};
use clap::Parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(author, version, about = "Nginx Prometheus Exporter", long_about = None)]
struct Args {
    #[arg(short, long, default_value = "/var/log/nginx/access.log")]
    log_path: PathBuf,

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

struct MetricsState {
    log_path: PathBuf,
    file_position: u64,
    metrics: HashMap<MetricLabels, Vec<f64>>,
}

impl MetricsState {
    fn new(log_path: PathBuf) -> Self {
        Self {
            log_path,
            file_position: 0,
            metrics: HashMap::new(),
        }
    }

    // TODO: rotation access.log file
    fn read_new_entries(&mut self) -> Result<HashMap<MetricLabels, Vec<f64>>, String> {
        let file =
            File::open(&self.log_path).map_err(|e| format!("Failed to open log file: {}", e))?;

        let mut reader = BufReader::new(file);

        reader
            .seek(SeekFrom::Start(self.file_position))
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

            self.file_position += bytes_read as u64;
            line.clear();
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

async fn metrics_handler(state: Arc<Mutex<MetricsState>>) -> (StatusCode, String) {
    println!("Access");
    let mut state = state.lock().unwrap();

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

    let mut output: Vec<String> = vec![
        "# HELP nginx_http_request_duration_seconds Request duration in seconds".to_string(),
        "# TYPE nginx_http_request_duration_seconds summary".to_string(),
    ];

    for (labels, durations) in metrics_map.iter() {
        let sum: f64 = durations.iter().sum();
        let count = durations.len();
        let mut sorted_durations = durations.clone();

        sorted_durations.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let p50 = calculate_quantile(&sorted_durations, 0.50);
        let p90 = calculate_quantile(&sorted_durations, 0.90);
        let p95 = calculate_quantile(&sorted_durations, 0.95);
        let p99 = calculate_quantile(&sorted_durations, 0.99);

        let label_str = format!(
            "method=\"{}\",path=\"{}\",status_code=\"{}\",host=\"{}\"",
            labels.method, labels.path, labels.status_code, labels.host
        );

        output.push(format!(
            "nginx_http_request_duration_seconds_sum{{{}}} {}",
            label_str, sum
        ));
        output.push(format!(
            "nginx_http_request_duration_seconds_count{{{}}} {}",
            label_str, count
        ));
        output.push(format!(
            "nginx_http_request_duration_seconds_p50{{{}}} {}",
            label_str, p50
        ));
        output.push(format!(
            "nginx_http_request_duration_seconds_p90{{{}}} {}",
            label_str, p90
        ));
        output.push(format!(
            "nginx_http_request_duration_seconds_p95{{{}}} {}",
            label_str, p95
        ));
        output.push(format!(
            "nginx_http_request_duration_seconds_p99{{{}}} {}",
            label_str, p99
        ));
    }

    (StatusCode::OK, output.join("\n"))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    println!("Starting Nginx Prometheus Exporter");
    println!("Log file: {:?}", args.log_path);
    println!("Server port: {}", args.port);

    let state = Arc::new(Mutex::new(MetricsState::new(args.log_path)));

    let app = Router::new().route(
        "/metrics",
        get({
            let state = Arc::clone(&state);
            move || metrics_handler(state)
        }),
    );

    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    println!("Server listening on {}", addr);

    axum::serve(listener, app)
        .await
        .expect("Server failed to start");
}
