import {homedir} from 'node:os';
import path from 'node:path';
import {execa, ResultPromise} from 'execa';
import getPort from 'get-port';
import {v1 as uuidv1} from 'uuid';

import {logger} from './logger';

/**
 * Default path to the `wash` binary -- by default we expect to be able to find it on the $PATH
 */
const DEFAULT_WASH_BIN_PATH = 'wash';

/** Amount of time to wait before force-kill `wash` subprocess */
const FORCE_KILL_WAIT_MS = 5000;

/** Default port on which `wash ui` will run */
const DEFAULT_WASH_UI_PORT = 3030;

/** Wash output format */
export enum WashOutputFormat {
  Text = 'text',
  Json = 'json',
}

export type WasmCloudInstanceNatsOptions = {
  /** Custom version for NATS cluster (this can trigger a download of NATS during `wash up`) */
  version?: string;
};

/** Options for a given wasmCloud instance */
export class WasmCloudInstanceOptions {
  /** Override path to `wash` binary (by default this is `wash`) */
  washBinPath?: string;
  /** Whether to start Wash UI */
  startWashUI?: boolean;
  /** Output format to use for wash logs */
  outputFormat?: WashOutputFormat;
  /** Options for NATS usage with the wasmCloud instance */
  nats?: WasmCloudInstanceNatsOptions;

  /**
   * An nkeys key (256-bit Ed25519 key) that the host uses to sign all invocations
   * see: https://www.npmjs.com/package/nkeys.js
   */
  clusterSeed?: Uint8Array;

  /** Labels (key, value) to apply to the host */
  labels?: Array<[string, string]>;

  /** Disable downloading of the wasmCloud host binary if not already installed */
  disableHostDownload?: boolean;

  /**
   * Allow starting multiple additonal hosts on the same machine at the same time
   * This is enabled by default.
   * */
  multiLocal?: boolean;
  /**
   * Whether to disable WADM
   * see: https://github.com/wasmCloud/wadm
   */
  disableWadm?: boolean;

  /** Options for `wash ui` invocation */
  washUI?: ProcessExtra;

  /** Whether to enable debug output/settings */
  debug?: boolean;

  /** Build a default instance of `WasmCloudInstanceOptions` */
  static default(): WasmCloudInstanceOptions {
    return {
      // By default, most tests should use the *latest* version of the `wash ui` code in-tree,
      // which means that starting `wash-ui` is unnecessary (use playwright's devServer instead)
      startWashUI: false,
      multiLocal: true,
      washBinPath: process.env.TEST_WASH_BIN_PATH ?? DEFAULT_WASH_BIN_PATH,
      debug: Boolean(process.env.DEBUG) ?? false,
    };
  }
}

/** Options to use while runnign `wash ui` */
export type ProcessExtra = {
  /**
   * Port on which to start wash UI
   * If this is not specified, one is randomly picked
   * */
  port?: number;
};

/** Process along with it's metadata */
type ProcessAndMeta = {
  stopped: boolean;
  process?: ResultPromise;
  abort?: AbortController;
  extra?: ProcessExtra;
};

/** Types of internal wash processes */
enum WashProcessType {
  Main = 'wash-main',
  UI = 'wash-ui',
}

/**
 * Instance of wasmCloud used for testing, generally managed with the WasmCloud Shell (`wash`)
 *
 * This instance is capable of spawning and controlling a wasmCloud host instance with `wash`, and
 * exposes utility functions for performing common operations.
 */
export class WasmCloudInstance {
  /** Options with which the wasmCloud instance should be created */
  opts: WasmCloudInstanceOptions;

  /** Unique ID of this wash instance */
  #_uuid: string;

  /** Internal processes of this instance */
  #processes: Map<WashProcessType, ProcessAndMeta>;

  get logPaths() {
    const logDirectory = path.join(homedir(), '.wash', 'downloads');

    return {
      wasmcloud: path.join(logDirectory, 'wasmcloud.log'),
      nats: path.join(logDirectory, 'nats.log'),
      wadm: path.join(logDirectory, 'wadm.log'),
    };
  }

  get pids() {
    const pids = new Map<WashProcessType, number>();
    for (const [type, processAndMeta] of this.#processes.entries()) {
      if (!processAndMeta.stopped) pids.set(type, processAndMeta.process?.pid ?? -1);
    }
    return pids;
  }

  constructor(options?: WasmCloudInstanceOptions) {
    this.opts = options ?? WasmCloudInstanceOptions.default();
    this.#_uuid = uuidv1();
    this.#processes = new Map();
  }

  /** Return the UUID for this instance */
  uuid(): string {
    return this.#_uuid;
  }

  /**
   * Start the wasmCloud instance with `wash`
   */
  async start(): Promise<void> {
    await this.startWashProcess();
    if (this.opts.startWashUI) {
      await this.startWashUIProcess();
    }
  }

  /**
   * Stop this WasmCloudInstance, along with all wash
   */
  async stop(): Promise<void> {
    await this.stopWashProcess();
    await this.stopWashUIProcess();
  }

  /** Base URL for the the running wash UI instance */
  uiBaseURL(): string {
    const existing = this.#processes.get(WashProcessType.UI);
    const port = existing ? existing.extra?.port : (this.opts.washUI?.port ?? DEFAULT_WASH_UI_PORT);
    return `http://localhost:${port}`;
  }

  protected getWashBinaryPath(): string {
    const washBinPath = this.opts.washBinPath ?? DEFAULT_WASH_BIN_PATH;
    logger.debug(`using wash binary @ [${washBinPath}]`);
    return washBinPath;
  }

  /** Start the wasmCloud host instance (with `wash`) */
  protected async startWashProcess(): Promise<void> {
    // Determine which arguments to pass to wash up
    const args = ['up', '--lattice', this.uuid(), '--label', `test-instance=${this.uuid()}`];

    // Customize output format if specifeid
    if (this.opts.outputFormat) {
      args.push('--output', this.opts.outputFormat);
    }

    // Customize NATS version if specifeid
    if (this.opts.nats?.version) {
      args.push('--nats-version', this.opts.nats.version);
    }

    // Set all provided labels if specified
    if (this.opts.labels) {
      for (const [key, value] of this.opts.labels) {
        args.push('--label', `${key}=${value}`);
      }
    }

    // Disable downloading the host if specified
    if (this.opts.disableHostDownload) {
      args.push('--wasmcloud-start-only');
    }

    // Disable WADM if specified
    if (this.opts.disableWadm) {
      args.push('--disable-wadm');
    }

    // Enable running multiple hosts on the same machine
    if (this.opts.multiLocal) {
      args.push('--multi-local');
    }

    // Start the wash process
    await this.startProcess(WashProcessType.Main, this.getWashBinaryPath(), args, {});

    // Start the actual `wash` process
    logger.debug('successfully started wash main process');
  }

  /** Generically start an internally managed process */
  protected async startProcess(
    processType: WashProcessType,
    binPath: string,
    args: string[],
    extra: ProcessExtra,
  ): Promise<ProcessAndMeta> {
    const existing = this.#processes.get(processType);
    if (existing && existing.process && !existing.stopped) {
      throw new Error('wash process is already running');
    }
    const abort = new AbortController();
    const child = execa(binPath, args, {
      reject: false,
      forceKillAfterDelay: FORCE_KILL_WAIT_MS,
      cancelSignal: abort.signal,
      stdout: this.opts.debug ? process.stdout : 'pipe',
      stderr: this.opts.debug ? process.stderr : 'pipe',
    });
    this.#processes.set(processType, {process: child, abort, stopped: false, extra});
    const processAndMeta = this.#processes.get(processType);
    if (!processAndMeta) {
      throw new Error('unexpectedly missing created process & metadata');
    }
    return processAndMeta;
  }

  /** Stop the running `wash` process */
  protected async stopWashProcess(): Promise<void> {
    // Best-effort use `wash down` to stop wash before stopping the OS process
    //
    // NOTE: Using `wash down` in this way fails consistently on in-development versions of wash,
    // due to missing $HOME/.wash/downloads/wasmcloud.pid.
    //
    // In versions of wash up to 0.27.0, attempting to use a normal signal to stop the process
    // will leave an orphaned host process. With fixes available in versions of wash > 0.27.0,
    // the extra `wash down` step is unnecessary (signal is properly handled with no orphaned host process)
    // so this step can be removed later (but probably should not be removed now).
    try {
      await execa(this.getWashBinaryPath(), ['down', '--lattice', this.uuid()], {
        forceKillAfterDelay: FORCE_KILL_WAIT_MS,
      });
    } catch (error) {
      logger.warn({err: error.toString()}, 'failed to stop existing lattice with `wash down`');
    }
    await this.stopProcess(WashProcessType.Main);
  }

  /** Start the `wash ui` process */
  protected async startWashUIProcess(): Promise<void> {
    // Determine which arguments to pass to wash up
    const args = ['ui'];

    // Figure out a port to use
    const port = this.opts.washUI?.port ?? (await getPort());
    args.push('--port', `${port}`);

    // Start the wash UI process
    await this.startProcess(WashProcessType.UI, this.getWashBinaryPath(), args, {
      port,
    });

    // Start the actual `wash` process
    logger.debug('successfully started wash UI process');
  }

  /** Stop the running `wash ui` process */
  protected async stopWashUIProcess(): Promise<void> {
    await this.stopProcess(WashProcessType.UI);
  }

  /** Generically stop an internal child process */
  protected async stopProcess(processType: WashProcessType): Promise<void> {
    const existing = this.#processes.get(processType);
    if (!existing) {
      return;
    }
    if (existing && existing.stopped) {
      return;
    }
    if (!existing.process) {
      throw new Error('unexpectedly missing process');
    }
    // Send kill
    await existing.process.kill('SIGTERM');
    existing.stopped = true;
  }
}
