import {expect} from '@playwright/test';

import {test} from '../common';

/// ////////
// Tests //
/// ////////

test('home page has the the proper title', async ({page, instance}) => {
  await page.goto(instance.uiBaseURL());
  await expect(page).toHaveTitle('wasmCloud UI | Washboard 🏄');
});
