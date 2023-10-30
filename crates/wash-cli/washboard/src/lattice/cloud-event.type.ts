export interface CloudEvent<T = unknown> {
  data: T;
  datacontenttype: string;
  id: string;
  source: string;
  specversion: string;
  time: string;
  type: string;
}
