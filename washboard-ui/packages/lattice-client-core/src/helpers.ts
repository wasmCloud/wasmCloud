import {type QueuedIterator} from 'nats.ws';
import {connect} from 'nats.ws';
import {
  type WasmCloudComponent,
  type WasmCloudHost,
  type WasmCloudProvider,
  type ApplicationStoreValue,
} from '@/types';

export function getManifestFrom(application: ApplicationStoreValue, version?: string) {
  const manifest = application.manifests[version ?? application.deployed_version];
  if (manifest === undefined) throw new Error('Manifest version not found');
  return manifest;
}

export async function toPromise<T>(iterator: QueuedIterator<T>): Promise<T[]> {
  const results = [];
  for await (const item of iterator) {
    results.push(item);
  }

  return results;
}

export async function canConnect(url: string): Promise<boolean> {
  try {
    const connection = await connect({servers: url});
    await connection.close();
    return true;
  } catch {
    return false;
  }
}

export function getCombinedInventoryFromHosts(hosts: Record<string, WasmCloudHost>) {
  const inventory: {
    components: Record<string, WasmCloudComponent>;
    providers: Record<string, WasmCloudProvider>;
  } = {
    components: {},
    providers: {},
  };

  for (const host of Object.values(hosts)) {
    for (const [key, component] of Object.entries(host.components)) {
      const existingComponent = inventory.components[key];
      if (existingComponent === undefined) {
        inventory.components[key] = {
          id: component.id,
          name: component.name ?? '',
          image_ref: component.image_ref,
          instances: [host.host_id],
          max_instances: component.max_instances,
          revision: component.revision,
          annotations: component.annotations ?? {},
        };
      } else {
        existingComponent.instances = [...(existingComponent.instances ?? []), host.host_id];
      }
    }

    for (const [key, provider] of Object.entries(host.providers)) {
      const existingProvider = inventory.providers[key];
      if (existingProvider === undefined) {
        inventory.providers[key] = {
          id: provider.id,
          name: provider.name,
          annotations: provider.annotations ?? {},
          image_ref: provider.image_ref ?? '',
          hosts: [host.host_id],
        };
      } else {
        existingProvider.hosts = [...(existingProvider.hosts ?? []), host.host_id];
      }
    }
  }

  return inventory;
}
