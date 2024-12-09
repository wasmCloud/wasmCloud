import {type Page, type Locator, expect} from '@playwright/test';

export class HomePage {
  readonly #connectionStatus: Locator;

  constructor(readonly page: Page) {
    this.#connectionStatus = this.page.getByTestId('connection-status');
  }

  async goto() {
    await this.page.goto('/');
  }

  async expectConnectionStatus(status: string, options?: {timeout?: number; ignoreCase?: boolean}) {
    await expect(this.#connectionStatus).toHaveAttribute('data-status', status, options);
  }

  async changeSettings(fields: Array<{label: string; value: string}>) {
    await this.openSettings();
    for (const {label, value} of fields) {
      await this.page.getByLabel(label).fill(value);
    }
    await this.saveSettings();
    await this.openSettings();
    for (const {label, value} of fields) {
      await expect(this.page.getByLabel(label)).toHaveValue(value);
    }
    await this.saveSettings();
  }

  async refreshLattice() {
    await this.openSettings();
    await this.saveSettings();
  }

  async openSettings() {
    await this.page.getByRole('button', {name: 'Settings'}).click();
    await expect(this.page.getByRole('dialog', {name: 'Settings'})).toBeVisible();
  }

  async saveSettings() {
    await this.page.getByRole('button', {name: 'Update'}).click();
    await expect(this.page.getByRole('dialog', {name: 'Settings'})).toBeHidden();
  }

  async hosts() {
    return this.page.getByTestId('host');
  }

  async components() {
    return this.page.getByTestId('component');
  }

  async providers() {
    return this.page.getByTestId('provider');
  }

  async links() {
    return this.page.getByTestId('link');
  }

  async configs() {
    return this.page.getByTestId('config');
  }
}
