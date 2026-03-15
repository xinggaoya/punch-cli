use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use prometheus::{Encoder, GaugeVec, IntGauge, IntGaugeVec, Opts, Registry, TextEncoder};

use crate::state::StateStore;
use crate::types::TunnelStatus;

#[derive(Clone)]
struct MetricsState {
    store: StateStore,
}

pub async fn serve_metrics(store: StateStore, port: u16) -> Result<()> {
    let state = MetricsState { store };
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("监听 metrics 端口失败: {port}"))?;

    println!("✓ Prometheus 指标已暴露: http://127.0.0.1:{port}/metrics");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("metrics 服务异常退出")?;
    Ok(())
}

async fn metrics_handler(State(state): State<MetricsState>) -> impl IntoResponse {
    match render_metrics(&state.store) {
        Ok(body) => (StatusCode::OK, body).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("render metrics failed: {error:#}"),
        )
            .into_response(),
    }
}

fn render_metrics(store: &StateStore) -> Result<String> {
    let records = store.refresh_statuses()?;

    let registry = Registry::new();
    let active = IntGauge::new("punch_active_tunnels", "当前活跃隧道数量")?;
    let total = IntGauge::new("punch_total_tunnels", "记录中的隧道总数")?;
    let up = IntGaugeVec::new(
        Opts::new("punch_tunnel_up", "隧道是否存活"),
        &["domain", "protocol"],
    )?;
    let port = GaugeVec::new(
        Opts::new("punch_tunnel_port", "本地端口"),
        &["domain", "protocol"],
    )?;

    registry.register(Box::new(active.clone()))?;
    registry.register(Box::new(total.clone()))?;
    registry.register(Box::new(up.clone()))?;
    registry.register(Box::new(port.clone()))?;

    total.set(records.len() as i64);
    active.set(
        records
            .iter()
            .filter(|record| record.status == TunnelStatus::Running)
            .count() as i64,
    );

    for record in records {
        let protocol = record.local_protocol.to_string();
        let domain = record.domain.as_str();
        up.with_label_values(&[domain, protocol.as_str()])
            .set((record.status == TunnelStatus::Running) as i64);
        port.with_label_values(&[domain, protocol.as_str()])
            .set(record.local_port as f64);
    }

    let metric_families = registry.gather();
    let mut buffer = Vec::new();
    TextEncoder::new().encode(&metric_families, &mut buffer)?;
    Ok(String::from_utf8(buffer)?)
}
