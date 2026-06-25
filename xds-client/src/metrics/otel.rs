//! OpenTelemetry [`MetricsRecorder`] implementation.
//!
//! Enabled by the `otel` Cargo feature. [`OtelMetricsRecorder`] adapts the
//! framework-agnostic [`MetricsRecorder`] trait onto an
//! [`opentelemetry::metrics::Meter`], so the gRFC A78 xDS client metrics flow
//! into whatever OpenTelemetry SDK the application has configured.
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use xds_client::{MetricsRecorder, OtelMetricsRecorder, XdsClient};
//!
//! let meter = opentelemetry::global::meter("grpc-xds");
//! let recorder: Arc<dyn MetricsRecorder> = Arc::new(OtelMetricsRecorder::new(meter));
//! let client = XdsClient::builder(config, transport, codec, runtime)
//!     .with_metrics_recorder(recorder)
//!     .build();
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter, UpDownCounter};

use super::{Instrument, InstrumentKind, KeyValue, MetricsRecorder, StringValue, Value};

/// An OpenTelemetry instrument, cached per metric descriptor.
#[derive(Debug)]
enum CachedInstrument {
    Counter(Counter<u64>),
    UpDownCounter(UpDownCounter<i64>),
    Histogram(Histogram<f64>),
    Gauge(Gauge<i64>),
}

/// A [`MetricsRecorder`] backed by an OpenTelemetry [`Meter`].
///
/// Every instrument in [`instruments::ALL`](super::instruments::ALL) is created
/// up front in [`new`](Self::new) and stored in an immutable map keyed by the
/// address of its `&'static Instrument` descriptor. Recording is therefore a
/// lock-free shared read of that map (mirroring grpc-go's stats-plugin design),
/// adding no synchronization to the xDS update path. Measurements for an
/// instrument that was not pre-registered are silently dropped.
///
/// The metric name, description, and unit are taken from the [`Instrument`]
/// descriptor, so backends observe the canonical gRFC A78 metadata.
#[derive(Debug)]
pub struct OtelMetricsRecorder {
    instruments: HashMap<usize, CachedInstrument>,
}

impl OtelMetricsRecorder {
    /// Create a recorder that emits measurements through `meter`.
    ///
    /// All instruments in [`instruments::ALL`](super::instruments::ALL) are
    /// eagerly created on the `meter`, so no instruments are built on the
    /// recording path.
    pub fn new(meter: Meter) -> Self {
        let mut instruments = HashMap::with_capacity(super::instruments::ALL.len());
        for &instrument in super::instruments::ALL {
            instruments.insert(key(instrument), build_instrument(&meter, instrument));
        }
        Self { instruments }
    }

    /// Look up the cached instrument for a descriptor, if it was registered.
    fn get(&self, instrument: &'static Instrument) -> Option<&CachedInstrument> {
        self.instruments.get(&key(instrument))
    }
}

/// Stable per-program cache key derived from the descriptor address.
fn key(instrument: &'static Instrument) -> usize {
    instrument as *const Instrument as usize
}

/// Build the OpenTelemetry instrument matching a descriptor's [`InstrumentKind`].
fn build_instrument(meter: &Meter, instrument: &'static Instrument) -> CachedInstrument {
    match instrument.kind {
        InstrumentKind::Counter => CachedInstrument::Counter(
            meter
                .u64_counter(instrument.name)
                .with_description(instrument.description)
                .with_unit(instrument.unit)
                .build(),
        ),
        InstrumentKind::UpDownCounter => CachedInstrument::UpDownCounter(
            meter
                .i64_up_down_counter(instrument.name)
                .with_description(instrument.description)
                .with_unit(instrument.unit)
                .build(),
        ),
        InstrumentKind::Histogram => CachedInstrument::Histogram(
            meter
                .f64_histogram(instrument.name)
                .with_description(instrument.description)
                .with_unit(instrument.unit)
                .build(),
        ),
        InstrumentKind::Gauge => CachedInstrument::Gauge(
            meter
                .i64_gauge(instrument.name)
                .with_description(instrument.description)
                .with_unit(instrument.unit)
                .build(),
        ),
    }
}

impl MetricsRecorder for OtelMetricsRecorder {
    fn add_counter_u64(&self, instrument: &'static Instrument, value: u64, attrs: &[KeyValue]) {
        if let Some(CachedInstrument::Counter(c)) = self.get(instrument) {
            c.add(value, &to_otel_attrs(attrs));
        }
    }

    fn add_up_down_counter_i64(
        &self,
        instrument: &'static Instrument,
        value: i64,
        attrs: &[KeyValue],
    ) {
        if let Some(CachedInstrument::UpDownCounter(c)) = self.get(instrument) {
            c.add(value, &to_otel_attrs(attrs));
        }
    }

    fn record_histogram_f64(
        &self,
        instrument: &'static Instrument,
        value: f64,
        attrs: &[KeyValue],
    ) {
        if let Some(CachedInstrument::Histogram(h)) = self.get(instrument) {
            h.record(value, &to_otel_attrs(attrs));
        }
    }

    fn record_gauge_i64(&self, instrument: &'static Instrument, value: i64, attrs: &[KeyValue]) {
        if let Some(CachedInstrument::Gauge(g)) = self.get(instrument) {
            g.record(value, &to_otel_attrs(attrs));
        }
    }
}

/// Convert the crate's attribute slice into OpenTelemetry key/value pairs.
fn to_otel_attrs(attrs: &[KeyValue]) -> Vec<opentelemetry::KeyValue> {
    attrs.iter().map(to_otel_key_value).collect()
}

fn to_otel_key_value(kv: &KeyValue) -> opentelemetry::KeyValue {
    let value = match &kv.value {
        Value::Bool(b) => opentelemetry::Value::Bool(*b),
        Value::Int(i) => opentelemetry::Value::I64(*i),
        Value::F64(f) => opentelemetry::Value::F64(*f),
        Value::Str(s) => opentelemetry::Value::String(to_otel_string(s)),
    };
    opentelemetry::KeyValue::new(kv.key, value)
}

fn to_otel_string(s: &StringValue) -> opentelemetry::StringValue {
    match s {
        StringValue::Static(st) => (*st).into(),
        StringValue::Owned(o) => o.to_string().into(),
        StringValue::RefCounted(r) => Arc::clone(r).into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::metrics::instruments;

    #[test]
    fn converts_each_value_variant() {
        let bool_kv = to_otel_key_value(&KeyValue::bool("k", true));
        assert_eq!(bool_kv.key.as_str(), "k");
        assert!(matches!(bool_kv.value, opentelemetry::Value::Bool(true)));

        let int_kv = to_otel_key_value(&KeyValue::int("k", 7));
        assert!(matches!(int_kv.value, opentelemetry::Value::I64(7)));

        let f64_kv = to_otel_key_value(&KeyValue::f64("k", 1.5));
        assert!(matches!(f64_kv.value, opentelemetry::Value::F64(v) if v == 1.5));

        let static_kv = to_otel_key_value(&KeyValue::str("k", "acked"));
        match static_kv.value {
            opentelemetry::Value::String(s) => assert_eq!(s.as_str(), "acked"),
            other => panic!("expected string, got {other:?}"),
        }

        let owned_kv = to_otel_key_value(&KeyValue::str("k", String::from("xds:///svc")));
        match owned_kv.value {
            opentelemetry::Value::String(s) => assert_eq!(s.as_str(), "xds:///svc"),
            other => panic!("expected string, got {other:?}"),
        }

        let arc_kv = to_otel_key_value(&KeyValue::str("k", Arc::<str>::from("server:443")));
        match arc_kv.value {
            opentelemetry::Value::String(s) => assert_eq!(s.as_str(), "server:443"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn registers_every_instrument_up_front() {
        // The default global meter is a no-op provider; the recorder must still
        // eagerly register one cached instrument per descriptor in `ALL`.
        let recorder = OtelMetricsRecorder::new(opentelemetry::global::meter("test"));
        assert_eq!(recorder.instruments.len(), instruments::ALL.len());
        for &instrument in instruments::ALL {
            assert!(
                recorder.get(instrument).is_some(),
                "{} not registered",
                instrument.name
            );
        }
    }

    #[test]
    fn recording_through_noop_meter_does_not_panic() {
        let recorder = OtelMetricsRecorder::new(opentelemetry::global::meter("test"));
        let attrs = [KeyValue::str("grpc.target", "xds:///svc")];

        recorder.add_counter_u64(&instruments::XDS_CLIENT_SERVER_FAILURE, 1, &attrs);
        recorder.add_counter_u64(&instruments::XDS_CLIENT_SERVER_FAILURE, 2, &attrs);
        recorder.add_up_down_counter_i64(&instruments::XDS_CLIENT_RESOURCES, -1, &attrs);
        recorder.record_gauge_i64(&instruments::XDS_CLIENT_CONNECTED, 1, &attrs);
    }
}
