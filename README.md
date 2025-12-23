# Nginx Prometheus Exporter

Prometheus exporter for nginx metrics that reads logs in JSON format and provides response time metrics.

## Features

- Read nginx logs in JSON format
- Track position in file (reads only new entries on each request)
- Calculate `nginx_http_request_duration_seconds` metrics:
  - `_sum` - total time of all requests
  - `_count` - number of requests
- Configurable log file path and server port

## Build

```bash
cargo build --release
```

## Usage

### Run with default parameters

```bash
cargo run --release
```

Defaults:
- Log path: `/var/log/nginx/access.log`
- Server port: `9090`

### Run with custom parameters

```bash
cargo run --release -- --log-path /path/to/nginx/access.log --port 9191
```

or

```bash
./target/release/nginx-prometheus-exporter -l /path/to/nginx/access.log -p 9191
```

### Command line parameters

- `-l, --log-path <LOG_PATH>` - path-pattern to nginx access-log files (default: `/var/log/nginx/*.log`)
- `-p, --port <PORT>` - HTTP server port (default: `9090`)
- `-h, --help` - show help
- `-V, --version` - show version

## Metrics format

The `/metrics` endpoint returns metrics in Prometheus histogram format with labels:

```
# HELP nginx_http_request_duration_seconds Request duration in seconds
# TYPE nginx_http_request_duration_seconds histogram
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.005"} 0
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.01"} 0
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.02"} 0
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.04"} 0
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.08"} 0
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.16"} 2
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.32"} 3
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="0.64"} 3
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="1.28"} 3
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="2.56"} 3
nginx_http_request_duration_seconds_bucket{method="GET",path="/api/users",status_code="2xx",host="api.example.com",le="+Inf"} 3
nginx_http_request_duration_seconds_sum{method="GET",path="/api/users",status_code="2xx",host="api.example.com"} 0.475
nginx_http_request_duration_seconds_count{method="GET",path="/api/users",status_code="2xx",host="api.example.com"} 3
```

### Labels

Each metric contains the following labels:
- `method` - HTTP request method (GET, POST, PUT, DELETE, etc.)
- `path` - URL path of the request
- `status_code` - HTTP response code grouped (1xx, 2xx, 3xx, 4xx, 5xx)
- `host` - hostname from the request

### Metric types

For each label combination, the exporter provides:
- **Histogram buckets** (`_bucket`) - exponential distribution with buckets [0.005, 0.01, 0.02, 0.04, 0.08, 0.16, 0.32, 0.64, 1.28, 2.56, +Inf] seconds
- **Sum** (`_sum`) - total time of all requests
- **Count** (`_count`) - number of requests

## Nginx log format

The exporter expects logs in JSON format, as specified in `nginx_log_format.conf`.
Critical field: `nginx.time.request` - request processing time in seconds.

## Testing

For testing, you can use the provided `test_access.log` file:

```bash
cargo run -- --log-path test_access.log --port 9191
```

Then in another terminal:

```bash
curl http://localhost:9191/metrics
```

## Prometheus configuration

Add to `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'nginx-exporter'
    static_configs:
      - targets: ['localhost:9090']
```

## Architecture

- **Position tracking**: the exporter stores the position of the last read byte in the file, so each request to `/metrics` processes only new entries
- **JSON parsing**: uses `serde_json` to parse nginx logs and extract necessary fields (method, path, status_code, host, request_time)
- **Label grouping**: metrics are grouped by unique label combinations (method, path, status_code, host) using HashMap
- **Histogram buckets**: uses exponential bucket distribution (ExponentialBuckets) with initial value 0.005s, factor 2.0 and 10 buckets, giving a range from 5ms to 2.56s
- **Quantile calculation**: quantiles (p50, p90, p95, p99) are calculated based on sorted data from the current set of new entries for each label group
- **Asynchronous HTTP server**: built on `axum` and `tokio`
