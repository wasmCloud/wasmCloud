import {test as base} from '@playwright/test';

import {logger} from './logger';
import {WasmCloudInstance, WasmCloudInstanceOptions} from './wasmcloud-instance';

export type WasmCloudInstanceFixture = {
  wasmCloud: WasmCloudInstance;
  wasmCloudOptions: WasmCloudInstanceOptions;
};

// Extend basic test by providing a wasmCloud instance that starts before
// the tests run and enables programmatic manipulation of a wasmCloud instance via `wash`
export const test = base.extend<WasmCloudInstanceFixture>({
  wasmCloudOptions: {},
  wasmCloud: [
    async ({wasmCloudOptions}, use, testInfo) => {
      const instance = new WasmCloudInstance({
        ...WasmCloudInstanceOptions.default(),
        ...wasmCloudOptions,
      });
      await test.step(`Start (${instance.uuid()})`, async () => {
        await instance.start();
        if (wasmCloudOptions.startWashUI) {
          const uiBaseURL = instance.uiBaseURL();
          logger.debug(`UI available at @ ${uiBaseURL}`);
          test.use({baseURL: uiBaseURL});
        }
      });

      await use(instance);

      await test.step(`Stop (${instance.uuid()})`, async () => {
        await instance.stop();

        // Attach logs if the test failed
        if (testInfo.status !== testInfo.expectedStatus) {
          for (const [name, logFile] of Object.entries(instance.logPaths)) {
            testInfo.attachments.push({name, contentType: 'text/plain', path: logFile});
          }
        }
      });
    },
    {title: 'wasmCloud', auto: true},
  ],
});
