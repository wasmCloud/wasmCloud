export type CloudEvent<T = unknown> = {
  data: T;
  datacontenttype: string;
  id: string;
  source: string;
  specversion: string;
  time: string;
  type: string;
};

export type WadmActor = {
  id: string;
  name: string;
  capabilities: string[];
  issuer: string;
  reference: string;
  instances: Record<
    string,
    {
      instance_id: string;
      annotations: Record<string, string>;
    }[]
  >;
};

export type WadmProvider = {
  id: string;
  name: string;
  issuer: string;
  contract_id: string;
  reference: string;
  link_name: string;
  hosts: Record<string, string>;
};

export type WadmLink = {
  actor_id: string;
  contract_id: string;
  link_name: string;
  public_key: string;
  provider_id: string;
};

export type WadmHost = {
  friendly_name: string;
  id: string;
  labels: Record<string, string>;
  annotations: Record<string, string>;
  last_seen: string;
  actors: Record<string, number>;
  providers: {
    contract_id: 'wasmcloud:httpserver';
    link_name: 'default';
    public_key: 'VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M';
    annotations: Record<string, string>;
  }[];
  uptime_seconds: number;
  version: string;
};
