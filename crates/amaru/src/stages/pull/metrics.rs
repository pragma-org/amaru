use crate::stages::metrics::Metric;

pub struct PullMetrics {
    pub header_size_bytes: u64,
}

impl Metric for PullMetrics {
    fn record(&self, meter: &opentelemetry::metrics::Meter) {
        let counter = meter
            .u64_counter("cardano_node_header_total_bytes")
            .with_description("Total bytes of headers received")
            .with_unit("int")
            .build();

        counter.add(self.header_size_bytes, &[]);
    }
}
