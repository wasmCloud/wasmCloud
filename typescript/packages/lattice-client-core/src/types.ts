import {type RequireOneOrNone, type LiteralUnion} from 'type-fest';

export type WadmApiResponse<Result = string, Data = never> = [Data] extends [never]
  ? {
      result: Result;
      message: string;
    }
  : {
      result: Result;
      message: string;
    } & Data;

export type ControlResponse<ResponseType = never> = [ResponseType] extends [never]
  ? {
      success: boolean;
      message: string;
    }
  : {
      success: boolean;
      message: string;
      response: ResponseType;
    };

export type WasmCloudProviderState = 'Pending' | 'Failed' | 'Running';
export type WasmCloudComponent = {
  id: string;
  name?: string;
  image_ref: string;
  instances: string[];
  annotations: Record<string, string>;
  max_instances: number;
  revision: number;
  state?: WasmCloudProviderState;
};

export type WasmCloudProvider = {
  id: string;
  name?: string;
  reference?: string;
  annotations: Record<string, string>;
  hosts: Record<string, WasmCloudProviderState>;
};

export type WasmCloudLink = {
  /** Source identifier for the link */
  source_id: string;
  /** Target for the link, which can be a unique identifier */
  target: string;
  /** Name of the link. Not providing this is equivalent to specifying "default" */
  name: string;
  /** WIT namespace of the link operation, e.g. `wasi` in `wasi:keyvalue/readwrite.get` */
  wit_namespace: string;
  /** WIT package of the link operation, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get` */
  wit_package: string;
  /** WIT Interfaces to be used for the link, e.g. `readwrite`, `atomic`, etc. */
  interfaces: string[];
  /** List of named configurations to provide to the source upon request */
  source_config: string[];
  /** List of named configurations to provide to the target upon request */
  target_config: string[];
};

export type WasmCloudConfig = {
  name: string;
  entries: Record<string, string>;
};

type ComponentDescription = {
  /** The component's unique identifier */
  id: string;
  /** The image reference for this component */
  image_ref: string;
  /** The name of the component, if one exists */
  name?: string;
  /** The annotations that were used in the start request that produced this component instance */
  annotations?: Record<string, string>;
  /** The revision number for this component instance */
  revision: number;
  /** The maximum number of concurrent requests this instance can handle */
  max_instances: number;
};

type ProviderDescription = {
  /** Provider's unique identifier */
  id: string;
  /** Image reference for this provider, if applicable */
  image_ref?: string;
  /** Name of the provider, if one exists */
  name?: string;
  /** The revision of the provider */
  revision: number;
  /** The annotations that were used in the start request that produced  this provider instance */
  annotations?: Record<string, string>;
};

export type WasmCloudHost = {
  /** components running on this host */
  components: ComponentDescription[];
  /** Providers running on this host */
  providers: ProviderDescription[];
  /** The host's unique ID */
  host_id: string;
  /** The host's human-readable friendly name */
  friendly_name: string;
  /** The host's labels */
  labels: Record<string, string>;
  /** The host's version */
  version: string;
  /** The host's uptime in human-readable format */
  uptime_human: string;
  /** The host's uptime in seconds */
  uptime_seconds: number;
};

export type WasmCloudHostRef = {
  /** The host's human-readable friendly name */
  friendly_name: string;
  /** The host's unique ID */
  id: string;
  /** providers */
  providers: Record<string, number>;
  /** components */
  components: Record<string, number>;
  /** The host's labels */
  labels: Record<
    LiteralUnion<'hostcore.arch' | 'hostcore.os' | 'hostcore.osfamily', string>,
    string
  >;
  /** The ID of the lattice that the host is connected to */
  lattice: string;
  /** The host's uptime in seconds */
  uptime_seconds: number;
  /** The host's version */
  version: string;
};

type StatusInfo = {
  type: DeploymentStatus;
  message?: string;
};

export type ApplicationStatus = {
  status: StatusInfo;
  scalers: Array<{
    status: StatusInfo;
    id: string;
    kind: string;
    name: string;
  }>;
  /** @deprecated get version data from parent ApplicationSummary */
  version: string;
  /** @deprecated get data from scalers */
  components: Array<{
    name: string;
    type: string;
    status: StatusInfo;
    traits: Array<{
      type: string;
      status: StatusInfo;
    }>;
  }>;
};

export type ApplicationDetail = {
  status: ApplicationStatus;
  versions: ApplicationHistory;
  manifest: ApplicationManifest;
};

export type ApplicationStoreValue = {
  manifests: Record<string, ApplicationManifest>;
  deployed_version: string;
};

export type ApplicationManifest = {
  apiVersion: string;
  kind: string;
  metadata: {
    name: string;
    annotations: Record<LiteralUnion<'version' | 'description', string>, string>;
  };
  spec: {
    components: ApplicationComponent[];
  };
};

export type ApplicationHistory = Array<{
  version: string;
  deployed: boolean;
}>;

export type ApplicationComponent = {
  name: string;
  type: 'component' | 'capability';
  properties: {
    image: string;
    id?: string;
    config?: Array<{
      name: string;
      properties: Record<string, string>;
    }>;
  };
  traits?: ApplicationTrait[];
};

export type ApplicationTrait =
  | ApplicationTraitSpreadScaler
  | ApplicationTraitDaemonScaler
  | ApplicationTraitLink;

export type ApplicationTraitSpreadScaler = {
  type: 'spreadscaler';
  properties: {
    instances: number;
    spread?: Array<{
      name: string;
      weight: number;
      requirements: Record<string, string>;
    }>;
  };
};

export type ApplicationTraitDaemonScaler = {
  type: 'daemonscaler';
  properties: {
    replicas: number;
    spread?: Array<{
      name: string;
      requirements: Record<string, string>;
    }>;
  };
};

export type ApplicationTraitLink = {
  type: 'link';
  properties: RequireOneOrNone<
    {
      target: string;
      namespace: string;
      package: string;
      interfaces: string[];
      target_config?: Array<{
        name: string;
        properties: Record<string, string>;
      }>;
      source_config?: Array<{
        name: string;
        properties: Record<string, string>;
      }>;
    },
    'source_config' | 'target_config'
  >;
};

export type ApplicationSummary = {
  name: string;
  version: string;
  description: string;
  deployed_version: string;
  detailed_status: ApplicationStatus;

  /** @deprecated Use detailed_status instead */
  status?: DeploymentStatus;
  /** @deprecated Use detailed_status instead */
  status_message?: string;
};

/** Application deployment status */
export type DeploymentStatus =
  | 'undeployed'
  | 'reconciling'
  | 'deployed'
  | 'failed'
  | 'waiting'
  | 'unknown';
