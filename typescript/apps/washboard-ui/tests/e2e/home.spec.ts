import {expect} from '@playwright/test';

import {test as base} from '../common';
import {HomePage} from './fixtures/home-page.fixture';

const test = base.extend<{homePage: HomePage}>({
  homePage: async ({page, wasmCloud}, use) => {
    const homePage = new HomePage(page);
    await homePage.goto();
    await homePage.expectConnectionStatus('Online');
    await homePage.changeSettings([{label: 'Lattice ID', value: wasmCloud.uuid()}]);
    await use(homePage);
  },
});

test('has the the proper title', async ({
  page,
  homePage, // eslint-disable-line @typescript-eslint/no-unused-vars -- including the fixture is what enables it
}) => {
  await expect(page).toHaveTitle('wasmCloud UI | Washboard 🏄');
});

test('shows host status', async ({homePage}) => {
  await expect(await homePage.hosts()).toHaveCount(1);
});
