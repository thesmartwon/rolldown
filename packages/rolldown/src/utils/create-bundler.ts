import { bindingifyInputOptions } from '../options/bindingify-input-options'
import { Bundler } from '../binding'
import { initializeParallelPlugins } from './initialize-parallel-plugins'
import { normalizeInputOptions } from './normalize-input-options'
import { normalizeOutputOptions } from './normalize-output-options'
import { bindingifyOutputOptions } from '../options/bindingify-output-options'
import { PluginDriver } from '../plugin/plugin-driver'
import type { InputOptions } from '../types/input-options'
import type { OutputOptions } from '../types/output-options'

export async function createBundler(
  inputOptions: InputOptions,
  outputOptions: OutputOptions,
): Promise<BundlerWithStopWorker> {
  const pluginDriver = new PluginDriver()
  inputOptions = await pluginDriver.callOptionsHook(inputOptions)
  // Convert `InputOptions` to `NormalizedInputOptions`.
  const normalizedInputOptions = await normalizeInputOptions(inputOptions)

  const parallelPluginInitResult = await initializeParallelPlugins(
    normalizedInputOptions.plugins,
  )

  try {
    outputOptions = pluginDriver.callOutputOptionsHook(
      normalizedInputOptions,
      outputOptions,
    )
    const normalizedOutputOptions = normalizeOutputOptions(outputOptions)

    // Convert `NormalizedInputOptions` to `BindingInputOptions`
    const bindingInputOptions = bindingifyInputOptions(
      normalizedInputOptions,
      normalizedOutputOptions,
    )

    return {
      bundler: new Bundler(
        bindingInputOptions,
        bindingifyOutputOptions(normalizedOutputOptions),
        parallelPluginInitResult?.registry,
      ),
      stopWorkers: parallelPluginInitResult?.stopWorkers,
    }
  } catch (e) {
    await parallelPluginInitResult?.stopWorkers()
    throw e
  }
}

export interface BundlerWithStopWorker {
  bundler: Bundler
  stopWorkers?: () => Promise<void>
}
