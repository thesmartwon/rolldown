use std::{
  ops::Deref,
  path::PathBuf,
  sync::{Arc, Weak},
};

use arcstr::ArcStr;
use dashmap::{DashMap, DashSet};
use rolldown_common::{ModuleInfo, ResolvedId, SharedFileEmitter, SharedNormalizedBundlerOptions};
use rolldown_resolver::{ResolveError, Resolver};

use crate::{
  types::{
    hook_resolve_id_skipped::HookResolveIdSkipped,
    plugin_context_resolve_options::PluginContextResolveOptions, plugin_idx::PluginIdx,
  },
  utils::resolve_id_with_plugins::resolve_id_check_external,
  PluginDriver,
};

#[derive(Debug, Clone)]
pub struct PluginContext(std::sync::Arc<PluginContextImpl>);

impl PluginContext {
  #[must_use]
  pub fn new_shared_with_skipped_resolve_calls(
    &self,
    skipped_resolve_calls: Vec<Arc<HookResolveIdSkipped>>,
  ) -> Self {
    Self(Arc::new(PluginContextImpl {
      skipped_resolve_calls,
      plugin_idx: self.plugin_idx,
      plugin_driver: Weak::clone(&self.plugin_driver),
      resolver: Arc::clone(&self.resolver),
      file_emitter: Arc::clone(&self.file_emitter),
      options: Arc::clone(&self.options),
      watch_files: Arc::clone(&self.watch_files),
      modules: Arc::clone(&self.modules),
    }))
  }
}

impl Deref for PluginContext {
  type Target = PluginContextImpl;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

#[derive(Debug)]
pub struct PluginContextImpl {
  pub(crate) skipped_resolve_calls: Vec<Arc<HookResolveIdSkipped>>,
  pub(crate) plugin_idx: PluginIdx,
  pub(crate) resolver: Arc<Resolver>,
  pub(crate) plugin_driver: Weak<PluginDriver>,
  pub(crate) file_emitter: SharedFileEmitter,
  pub(crate) options: SharedNormalizedBundlerOptions,
  pub(crate) watch_files: Arc<DashSet<ArcStr>>,
  pub(crate) modules: Arc<DashMap<ArcStr, Arc<ModuleInfo>>>,
}

impl From<PluginContextImpl> for PluginContext {
  fn from(ctx: PluginContextImpl) -> Self {
    Self(Arc::new(ctx))
  }
}

impl PluginContextImpl {
  pub async fn resolve(
    &self,
    specifier: &str,
    importer: Option<&str>,
    extra_options: Option<PluginContextResolveOptions>,
  ) -> anyhow::Result<Result<ResolvedId, ResolveError>> {
    let plugin_driver = self
      .plugin_driver
      .upgrade()
      .ok_or_else(|| anyhow::format_err!("Plugin driver is already dropped."))?;

    let normalized_extra_options = extra_options.unwrap_or_default();

    resolve_id_check_external(
      &self.resolver,
      &plugin_driver,
      specifier,
      importer,
      false,
      normalized_extra_options.import_kind,
      if normalized_extra_options.skip_self {
        let mut skipped_resolve_calls = Vec::with_capacity(self.skipped_resolve_calls.len() + 1);
        skipped_resolve_calls.extend(self.skipped_resolve_calls.clone());
        skipped_resolve_calls.push(Arc::new(HookResolveIdSkipped {
          plugin_idx: self.plugin_idx,
          importer: importer.map(Into::into),
          specifier: specifier.into(),
        }));
        Some(skipped_resolve_calls)
      } else if !self.skipped_resolve_calls.is_empty() {
        Some(self.skipped_resolve_calls.clone())
      } else {
        None
      },
      normalized_extra_options.custom,
      false,
      &self.options,
    )
    .await
  }

  pub fn emit_file(&self, file: rolldown_common::EmittedAsset) -> ArcStr {
    self.file_emitter.emit_file(file)
  }

  pub fn try_get_file_name(&self, reference_id: &str) -> Result<ArcStr, String> {
    self.file_emitter.try_get_file_name(reference_id)
  }

  pub fn get_file_name(&self, reference_id: &str) -> ArcStr {
    self.file_emitter.get_file_name(reference_id)
  }

  pub fn get_module_info(&self, module_id: &str) -> Option<Arc<rolldown_common::ModuleInfo>> {
    // TODO external module
    self.modules.get(module_id).map(|v| Arc::<rolldown_common::ModuleInfo>::clone(v.value()))
  }

  pub fn get_module_ids(&self) -> Vec<String> {
    // TODO external module
    self.modules.iter().map(|v| v.key().to_string()).collect()
  }

  pub fn cwd(&self) -> &PathBuf {
    self.resolver.cwd()
  }

  pub fn add_watch_file(&self, file: &str) {
    self.watch_files.insert(file.into());
  }
}
