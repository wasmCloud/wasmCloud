import {expect} from '@playwright/test';

import {test} from '../common';

test('home page has the the proper title', async ({page}) => {
  await page.goto('/');
  await expect(page).toHaveTitle('wasmCloud UI | Washboard 🏄');
});

test('page shows the application frame', async ({page}) => {
  await page.goto('/');
  const rootElement = await page.$('#root');
  await expect(await rootElement.innerHTML()).toBeTruthy();
});
