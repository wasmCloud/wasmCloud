export type CloudEvent<T = unknown> = {
  data: T;
  datacontenttype: string;
  id: string;
  source: string;
  specversion: string;
  time: string;
  type: string;
};

export type TopicResponse<ResponseType = any> = {
  success: boolean;
  message: string;
  response: ResponseType;
};

export type LinkResponse = TopicResponse<WadmLink[]>

export type WadmComponent = {
  id: string;
  name: string;
  issuer: string;
  reference: string;
  instances: Record<
    string,
    {
      count: number;
      annotations: Record<string, string>;
    }[]
  >;
};

export type WadmProvider = {
  id: string;
  name: string;
  issuer: string;
  reference: string;
  hosts: Record<string, string>;
};

export type WadmLink = {
  source_id: string;
  target: string;
  name: string;
  wit_namespace: string;
  wit_package: string;
  interfaces: string[];
  source_config: string[];
  target_config: string[];
};

export type WadmConfig = {
  name: string;
  entries: Record<string, string>;
}

export type WadmHost = {
  friendly_name: string;
  id: string;
  labels: Record<string, string>;
  annotations: Record<string, string>;
  last_seen: string;
  components: Record<string, number>;
  providers: {
    contract_id: 'wasmcloud:httpserver';
    link_name: 'default';
    public_key: 'VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M';
    annotations: Record<string, string>;
  }[];
  uptime_seconds: number;
  version: string;
};
