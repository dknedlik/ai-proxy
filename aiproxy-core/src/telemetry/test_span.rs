#![cfg(test)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{span, Subscriber};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};
use tracing_subscriber::registry;
use tracing_core::field::{Field, Visit};

#[derive(Default, Debug)]
pub struct SpanData {
    pub name: String,
    pub fields: Mutex<HashMap<String, String>>, // stringified values via Debug
}

#[derive(Default, Debug)]
pub struct SpanStore {
    pub spans: Mutex<HashMap<tracing::span::Id, Arc<SpanData>>>,
}

#[derive(Clone)]
pub struct CaptureLayer {
    pub store: Arc<SpanStore>,
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, _ctx: Context<'_, S>) {
        let name = attrs.metadata().name().to_string();
        let data = Arc::new(SpanData { name, ..Default::default() });

        // Seed with any initial attributes recorded at creation
        {
            let mut map = data.fields.lock().unwrap();
            struct MapVisitor<'a> {
                map: &'a mut HashMap<String, String>,
            }
            impl<'a> Visit for MapVisitor<'a> {
                fn record_debug(&mut self, field: &Field, value: &dyn core::fmt::Debug) {
                    self.map
                        .insert(field.name().to_string(), format!("{value:?}"));
                }
                fn record_i64(&mut self, field: &Field, value: i64) {
                    self.map.insert(field.name().to_string(), value.to_string());
                }
                fn record_u64(&mut self, field: &Field, value: u64) {
                    self.map.insert(field.name().to_string(), value.to_string());
                }
                fn record_bool(&mut self, field: &Field, value: bool) {
                    self.map.insert(field.name().to_string(), value.to_string());
                }
                fn record_str(&mut self, field: &Field, value: &str) {
                    self.map
                        .insert(field.name().to_string(), format!("\"{}\"", value));
                }
                fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
                    self.map.insert(field.name().to_string(), format!("{value}"));
                }
            }
            attrs.record(&mut MapVisitor { map: &mut map });
        }

        self.store.spans.lock().unwrap().insert(id.clone(), data);
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, _ctx: Context<'_, S>) {
        if let Some(data) = self.store.spans.lock().unwrap().get(id) {
            let mut map = data.fields.lock().unwrap();
            struct MapVisitor<'a> {
                map: &'a mut HashMap<String, String>,
            }
            impl<'a> Visit for MapVisitor<'a> {
                fn record_debug(&mut self, field: &Field, value: &dyn core::fmt::Debug) {
                    self.map
                        .insert(field.name().to_string(), format!("{value:?}"));
                }
                fn record_i64(&mut self, field: &Field, value: i64) {
                    self.map.insert(field.name().to_string(), value.to_string());
                }
                fn record_u64(&mut self, field: &Field, value: u64) {
                    self.map.insert(field.name().to_string(), value.to_string());
                }
                fn record_bool(&mut self, field: &Field, value: bool) {
                    self.map.insert(field.name().to_string(), value.to_string());
                }
                fn record_str(&mut self, field: &Field, value: &str) {
                    self.map
                        .insert(field.name().to_string(), format!("\"{}\"", value));
                }
                fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
                    self.map.insert(field.name().to_string(), format!("{value}"));
                }
            }
            values.record(&mut MapVisitor { map: &mut map });
        }
    }
}

static GUARDS: once_cell::sync::Lazy<Mutex<Vec<tracing::subscriber::DefaultGuard>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

pub fn install_capture() -> Arc<SpanStore> {
    use tracing_subscriber::prelude::*;
    let store = Arc::new(SpanStore::default());
    let layer = CaptureLayer { store: store.clone() };
    let subscriber = registry::Registry::default().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);
    GUARDS.lock().unwrap().push(guard);
    store
}
