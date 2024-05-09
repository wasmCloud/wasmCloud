export type CloudEvent<EventType extends LatticeEventType, DataType = unknown> = [
  DataType,
] extends [never]
  ? {
      id: string;
      type: EventType;
      source: string;
      datacontenttype: string;
      specversion: string;
      time: string;
    }
  : {
      id: string;
      type: EventType;
      source: string;
      datacontenttype: string;
      specversion: string;
      time: string;
      data: DataType;
    };

export enum LatticeEventType {
  ComponentScaled = 'component_scaled',
  ComponentScaleFailed = 'component_scale_failed',
  LinkDefinitionSet = 'linkdef_set',
  LinkDefinitionDeleted = 'linkdef_deleted',
  ProviderStarted = 'provider_started',
  ProviderStartFailed = 'provider_start_failed',
  ProviderStopped = 'provider_stopped',
  HealthCheckPassed = 'health_check_passed',
  HealthCheckFailed = 'health_check_failed',
  HealthCheckStatus = 'health_check_status',
  ConfigSet = 'config_set',
  ConfigDeleted = 'config_deleted',
  LabelsChanged = 'labels_changed',
  HostHeartbeat = 'host_heartbeat',
  HostStarted = 'host_started',
  HostStopped = 'host_stopped',
}

export type ComponentScaledEvent =
  | CloudEvent<
      LatticeEventType.ComponentScaled,
      {
        annotations: Record<string, string>;
        host_id: string;
        image_ref: string;
        max_instances: number;
        component_id: string;
      }
    >
  | CloudEvent<
      LatticeEventType.ComponentScaled,
      {
        public_key: string;
        claims: string;
        annotations: Record<string, string>;
        host_id: string;
        image_ref: string;
        max_instances: number;
        component_id: string;
      }
    >;

export type ComponentScaleFailedEvent =
  | CloudEvent<
      LatticeEventType.ComponentScaleFailed,
      {
        annotations: Record<string, string>;
        host_id: string;
        image_ref: string;
        component_id: string;
        max_instances: number;
        error: string;
      }
    >
  | CloudEvent<
      LatticeEventType.ComponentScaleFailed,
      {
        annotations: Record<string, string>;
        host_id: string;
        image_ref: string;
        component_id: string;
        max_instances: number;
        error: string;
        public_key: string;
      }
    >;

export type LinkDefinitionSetEvent = CloudEvent<
  LatticeEventType.LinkDefinitionSet,
  {
    source_id: string;
    target: string;
    name: string;
    wit_namespace: string;
    wit_package: string;
    interfaces: string[];
    source_config: string[];
    target_config: string[];
  }
>;

export type LinkDefinitionDeletedEvent = CloudEvent<
  LatticeEventType.LinkDefinitionDeleted,
  {
    source_id: string;
    name: string;
    wit_namespace: string;
    wit_package: string;
  }
>;

export type ProviderStartedEvent =
  | CloudEvent<
      LatticeEventType.ProviderStarted,
      {
        host_id: string;
        image_ref: string;
        provider_id: string;
        annotations: Record<string, string>;
      }
    >
  | CloudEvent<
      LatticeEventType.ProviderStarted,
      {
        host_id: string;
        image_ref: string;
        provider_id: string;
        annotations: Record<string, string>;
        claims: {
          issuer: string;
          tags: string[];
          name: string;
          version: string;
          not_before_human: string;
          expires_human: string;
        };
      }
    >;

export type ProviderStartFailedEvent = CloudEvent<
  LatticeEventType.ProviderStartFailed,
  {
    provider_ref: string;
    provider_id: string;
    error: string;
  }
>;

export type ProviderStoppedEvent = CloudEvent<
  LatticeEventType.ProviderStopped,
  {
    host_id: string;
    provider_id: string;
    annotations: Record<string, string>;
    reason: string;
  }
>;

export type HealthCheckPassedEvent = CloudEvent<
  LatticeEventType.HealthCheckPassed,
  {
    host_id: string;
    provider_id: string;
  }
>;

export type HealthCheckFailedEvent = CloudEvent<
  LatticeEventType.HealthCheckFailed,
  {
    host_id: string;
    provider_id: string;
  }
>;

export type HealthCheckStatusEvent = CloudEvent<
  LatticeEventType.HealthCheckStatus,
  {
    host_id: string;
    provider_id: string;
  }
>;

export type ConfigSetEvent = CloudEvent<
  LatticeEventType.ConfigSet,
  {
    config_name: string;
  }
>;

export type ConfigDeletedEvent = CloudEvent<
  LatticeEventType.ConfigDeleted,
  {
    config_name: string;
  }
>;

export type LabelsChangedEvent = CloudEvent<
  LatticeEventType.LabelsChanged,
  {
    host_id: string;
    labels: Record<string, string>;
  }
>;

export type HostHeartbeatEvent = CloudEvent<
  LatticeEventType.HostHeartbeat,
  {
    host_id: string;
    uptime_seconds: number;
  }
>;

export type HostStartedEvent = CloudEvent<
  LatticeEventType.HostStarted,
  {
    host_id: string;
    labels: Record<string, string>;
  }
>;

export type HostStoppedEvent = CloudEvent<
  LatticeEventType.HostStopped,
  {
    host_id: string;
    reason: string;
  }
>;

export type LatticeEvent =
  | ComponentScaledEvent
  | ComponentScaleFailedEvent
  | LinkDefinitionSetEvent
  | LinkDefinitionDeletedEvent
  | ProviderStartedEvent
  | ProviderStartFailedEvent
  | ProviderStoppedEvent
  | HealthCheckPassedEvent
  | HealthCheckFailedEvent
  | HealthCheckStatusEvent
  | ConfigSetEvent
  | ConfigDeletedEvent
  | LabelsChangedEvent
  | HostHeartbeatEvent
  | HostStartedEvent
  | HostStoppedEvent;
