/* eslint-disable react-hooks/rules-of-hooks -- Disabling rules-of-hooks since these are Playwright extensions */

import {test as base} from '@playwright/test';

import {logger} from './logger';
import {WasmCloudInstance, WasmCloudInstanceOptions} from './wasmcloud-instance';

export type TestMeta = {instance: WasmCloudInstance};

// Extend basic test by providing a wasmCloud instance that starts before
// the tests run and enables programmatic manipulation of a wasmCloud instance via `wash`
export const test = base.extend<TestMeta>({
  instance: async ({page}, use) => {
    const instance = new WasmCloudInstance({
      ...WasmCloudInstanceOptions.default(),
      washUI: {
        // NOTE: this port is set to the deafult for the `webServer` configured via playwright config
        port: 5173,
      },
    });
    await instance.start();
    const uiBaseURL = instance.uiBaseURL();
    logger.debug(`UI available at @ ${uiBaseURL}]`);
    await page.goto(uiBaseURL);
    await use(instance);
    await instance.stop();
  },
});
