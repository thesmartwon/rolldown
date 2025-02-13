use crate::{
  types::generator::{GenerateContext, GenerateOutput, Generator},
  utils::{chunk::generate_rendered_chunk, render_ecma_module::render_ecma_module},
};

use anyhow::Result;
use rolldown_common::{
  EcmaAssetMeta, InstantiatedChunk, InstantiationKind, ModuleId, ModuleIdx, OutputFormat,
  RenderedModule,
};
use rolldown_error::BuildResult;
use rolldown_plugin::HookAddonArgs;
use rolldown_sourcemap::Source;
#[cfg(not(target_family = "wasm"))]
use rolldown_utils::rayon::IndexedParallelIterator;
use rolldown_utils::rayon::{IntoParallelRefIterator, ParallelIterator};
use rustc_hash::FxHashMap;
use sugar_path::SugarPath;

use super::format::{
  app::render_app, cjs::render_cjs, esm::render_esm, iife::render_iife, umd::render_umd,
};

pub type RenderedModuleSources = Vec<(ModuleIdx, ModuleId, Option<Vec<Box<dyn Source + Send>>>)>;

pub struct EcmaGenerator;

impl Generator for EcmaGenerator {
  #[allow(clippy::too_many_lines)]
  async fn instantiate_chunk<'a>(
    ctx: &mut GenerateContext<'a>,
  ) -> Result<BuildResult<GenerateOutput>> {
    let mut rendered_modules = FxHashMap::default();
    let module_id_to_codegen_ret = std::mem::take(&mut ctx.module_id_to_codegen_ret);
    let rendered_module_sources = ctx
      .chunk
      .modules
      .par_iter()
      .copied()
      .zip(module_id_to_codegen_ret)
      .filter_map(|(id, codegen_ret)| {
        ctx.link_output.module_table.modules[id]
          .as_normal()
          .map(|m| (m, codegen_ret.expect("should have codegen_ret")))
      })
      .map(|(m, codegen_ret)| {
        (m.idx, m.id.clone(), render_ecma_module(m, ctx.options, codegen_ret))
      })
      .collect::<Vec<_>>();

    rendered_module_sources.iter().for_each(|(_, module_id, _)| {
      // FIXME: NAPI-RS used CStr under the hood, so it can't handle null byte in the string.
      if !module_id.starts_with('\0') {
        rendered_modules.insert(module_id.clone(), RenderedModule { code: None });
      }
    });

    let rendered_chunk = generate_rendered_chunk(
      ctx.chunk,
      rendered_modules,
      ctx.chunk.pre_rendered_chunk.as_ref().expect("Should have pre-rendered chunk"),
      ctx.chunk_graph,
    );
    let hashbang = match ctx.chunk.kind {
      rolldown_common::ChunkKind::EntryPoint { is_user_defined, module, .. } if is_user_defined => {
        let module = &ctx.link_output.module_table.modules[module];
        match module {
          rolldown_common::Module::Normal(normal_module) => {
            let source = &normal_module.source;
            normal_module
              .ecma_view
              .hashbang_range
              .map(|range| &source.as_str()[range.start as usize..range.end as usize])
          }
          rolldown_common::Module::External(_) => None,
        }
      }
      _ => None,
    };

    let banner = {
      let injection = match ctx.options.banner.as_ref() {
        Some(hook) => hook.call(&rendered_chunk).await?,
        None => None,
      };
      ctx
        .plugin_driver
        .banner(HookAddonArgs { chunk: &rendered_chunk }, injection.unwrap_or_default())
        .await?
    };

    let intro = {
      let injection = match ctx.options.intro.as_ref() {
        Some(hook) => hook.call(&rendered_chunk).await?,
        None => None,
      };
      ctx
        .plugin_driver
        .intro(HookAddonArgs { chunk: &rendered_chunk }, injection.unwrap_or_default())
        .await?
    };

    let outro = {
      let injection = match ctx.options.outro.as_ref() {
        Some(hook) => hook.call(&rendered_chunk).await?,
        None => None,
      };
      ctx
        .plugin_driver
        .outro(HookAddonArgs { chunk: &rendered_chunk }, injection.unwrap_or_default())
        .await?
    };

    let footer = {
      let injection = match ctx.options.footer.as_ref() {
        Some(hook) => hook.call(&rendered_chunk).await?,
        None => None,
      };
      ctx
        .plugin_driver
        .footer(HookAddonArgs { chunk: &rendered_chunk }, injection.unwrap_or_default())
        .await?
    };

    let source_joiner = match ctx.options.format {
      OutputFormat::Esm => render_esm(
        ctx,
        &rendered_module_sources,
        banner.as_deref(),
        footer.as_deref(),
        intro.as_deref(),
        outro.as_deref(),
        hashbang,
      ),
      OutputFormat::Cjs => {
        match render_cjs(
          ctx,
          &rendered_module_sources,
          banner.as_deref(),
          footer.as_deref(),
          intro.as_deref(),
          outro.as_deref(),
          hashbang,
        ) {
          Ok(source_joiner) => source_joiner,
          Err(errors) => return Ok(Err(errors)),
        }
      }
      OutputFormat::App => render_app(
        ctx,
        &rendered_module_sources,
        banner.as_deref(),
        footer.as_deref(),
        intro.as_deref(),
        outro.as_deref(),
        hashbang,
      ),
      OutputFormat::Iife => {
        match render_iife(
          ctx,
          &rendered_module_sources,
          banner.as_deref(),
          footer.as_deref(),
          intro.as_deref(),
          outro.as_deref(),
          hashbang,
        ) {
          Ok(source_joiner) => source_joiner,
          Err(errors) => return Ok(Err(errors)),
        }
      }
      OutputFormat::Umd => {
        match render_umd(
          ctx,
          &rendered_module_sources,
          banner.as_deref(),
          footer.as_deref(),
          intro.as_deref(),
          outro.as_deref(),
        ) {
          Ok(source_joiner) => source_joiner,
          Err(errors) => return Ok(Err(errors)),
        }
      }
    };

    let (content, mut map) = source_joiner.join();

    // Here file path is generated by chunk file name template, it maybe including path segments.
    // So here need to read it's parent directory as file_dir.
    let file_path = ctx.options.cwd.as_path().join(&ctx.options.dir).join(
      ctx
        .chunk
        .preliminary_filename
        .as_deref()
        .expect("chunk file name should be generated before rendering")
        .as_str(),
    );
    let file_dir = file_path.parent().expect("chunk file name should have a parent");

    if let Some(map) = map.as_mut() {
      let paths =
        map.get_sources().map(|source| source.as_path().relative(file_dir)).collect::<Vec<_>>();
      // Here not normalize the windows path, the rollup `sourcemap_path_transform` ctx.options need to original path.
      let sources = paths.iter().map(|x| x.to_string_lossy()).collect::<Vec<_>>();
      map.set_sources(sources.iter().map(std::convert::AsRef::as_ref).collect::<Vec<_>>());
    }

    Ok(Ok(GenerateOutput {
      chunks: vec![InstantiatedChunk {
        origin_chunk: ctx.chunk_idx,
        content: content.into(),
        map,
        kind: InstantiationKind::from(EcmaAssetMeta { rendered_chunk }),
        augment_chunk_hash: None,
        file_dir: file_dir.to_path_buf(),
        preliminary_filename: ctx
          .chunk
          .preliminary_filename
          .clone()
          .expect("should have preliminary filename"),
      }],
      warnings: std::mem::take(&mut ctx.warnings),
    }))
  }
}
