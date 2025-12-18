# Nginx Prometheus Exporter

Prometheus exporter для метрик nginx, который читает логи в JSON формате и предоставляет метрики времени ответа.

## Возможности

- Чтение логов nginx в JSON формате
- Отслеживание позиции в файле (читает только новые записи при каждом запросе)
- Вычисление метрик `nginx_http_request_duration_seconds`:
  - `_sum` - сумма времени всех запросов
  - `_count` - количество запросов
  - Квантили: p50, p90, p95, p99
- Конфигурируемый путь к лог-файлу и порт сервера

## Сборка

```bash
cargo build --release
```

## Использование

### Запуск с параметрами по умолчанию

```bash
cargo run --release
```

По умолчанию:
- Путь к логу: `/var/log/nginx/access.log`
- Порт сервера: `9090`

### Запуск с пользовательскими параметрами

```bash
cargo run --release -- --log-path /path/to/nginx/access.log --port 9191
```

или

```bash
./target/release/frontend-infra-nginx-exporter -l /path/to/nginx/access.log -p 9191
```

### Параметры командной строки

- `-l, --log-path <LOG_PATH>` - путь к файлу логов nginx (по умолчанию: `/var/log/nginx/access.log`)
- `-p, --port <PORT>` - порт HTTP сервера (по умолчанию: `9090`)
- `-h, --help` - показать справку
- `-V, --version` - показать версию

## Формат метрик

Эндпоинт `/metrics` возвращает метрики в формате Prometheus с лейблами:

```
# HELP nginx_http_request_duration_seconds Request duration in seconds
# TYPE nginx_http_request_duration_seconds summary
nginx_http_request_duration_seconds_sum{method="GET",path="/api/users",status_code="200",host="api.example.com"} 0.275
nginx_http_request_duration_seconds_count{method="GET",path="/api/users",status_code="200",host="api.example.com"} 2
nginx_http_request_duration_seconds{method="GET",path="/api/users",status_code="200",host="api.example.com",quantile="0.5"} 0.15
nginx_http_request_duration_seconds{method="GET",path="/api/users",status_code="200",host="api.example.com",quantile="0.9"} 0.15
nginx_http_request_duration_seconds{method="GET",path="/api/users",status_code="200",host="api.example.com",quantile="0.95"} 0.15
nginx_http_request_duration_seconds{method="GET",path="/api/users",status_code="200",host="api.example.com",quantile="0.99"} 0.15
```

### Лейблы

Каждая метрика содержит следующие лейблы:
- `method` - HTTP метод запроса (GET, POST, PUT, DELETE и т.д.)
- `path` - URL путь запроса
- `status_code` - HTTP код ответа (200, 404, 500 и т.д.)
- `host` - имя хоста из запроса

## Формат логов nginx

Экспортер ожидает логи в JSON формате, как указано в `nginx_log_format.conf`.
Критически важное поле: `nginx.time.request` - время обработки запроса в секундах.

## Тестирование

Для тестирования можно использовать предоставленный файл `test_access.log`:

```bash
cargo run -- --log-path test_access.log --port 9191
```

Затем в другом терминале:

```bash
curl http://localhost:9191/metrics
```

## Конфигурация Prometheus

Добавьте в `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'nginx-exporter'
    static_configs:
      - targets: ['localhost:9090']
```

## Архитектура

- **Отслеживание позиции**: экспортер хранит позицию последнего прочитанного байта в файле, поэтому при каждом запросе к `/metrics` обрабатываются только новые записи
- **Парсинг JSON**: использует `serde_json` для парсинга логов nginx и извлечения необходимых полей (method, path, status_code, host, request_time)
- **Группировка по лейблам**: метрики группируются по уникальным комбинациям лейблов (method, path, status_code, host) с использованием HashMap
- **Вычисление квантилей**: квантили вычисляются на основе отсортированных данных из текущего набора новых записей для каждой группы лейблов
- **Асинхронный HTTP сервер**: построен на `axum` и `tokio`
