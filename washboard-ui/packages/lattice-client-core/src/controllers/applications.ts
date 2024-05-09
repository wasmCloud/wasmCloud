import {BaseController} from '@/controllers/base-controller';
import {
  type ApplicationStatus,
  type ApplicationHistory,
  type ApplicationManifest,
  type ApplicationSummary,
  type WadmApiResponse,
  ApplicationDetail,
} from '@/types';

type ApplicationListResponse =
  | {
      result: 'success';
      message: string;
      models: ApplicationSummary[];
    }
  | {
      result: 'error';
      message: string;
    };

type ManifestResponseResult = 'success' | 'notfound' | 'error';
type ApplicationManifestResponse = {
  [Result in ManifestResponseResult]: Result extends 'success'
    ? WadmApiResponse<Result, {manifest: ApplicationManifest}>
    : WadmApiResponse<Result>;
}[ManifestResponseResult];

type ApplicationHistoryResponse =
  | WadmApiResponse<'success', {versions: ApplicationHistory}>
  | WadmApiResponse<'error'>
  | WadmApiResponse<'notfound', {versions: never[]}>;

type StatusResponseResult = 'ok' | 'error' | 'notfound';
type ApplicationStatusResponse = {
  [Result in StatusResponseResult]: Result extends 'ok'
    ? WadmApiResponse<Result, {status: ApplicationStatus}>
    : WadmApiResponse<Result, {message: string}>;
}[StatusResponseResult];

type DeleteResponseResult = 'deleted' | 'error' | 'noop';
type ApplicationDeleteResponse = {
  [Result in DeleteResponseResult]: WadmApiResponse<Result>;
}[DeleteResponseResult];

type PutResponseResult = 'error' | 'created' | 'newversion';
type ApplicationPutResponse = {
  [Result in PutResponseResult]: Result extends 'error'
    ? WadmApiResponse<Result>
    : WadmApiResponse<Result, {name: string; current_version: string; total_versions: number}>;
}[PutResponseResult];

type DeployResponseResult = 'error' | 'acknowledged' | 'notfound';
type ApplicationDeployResponse = {
  [Result in DeployResponseResult]: WadmApiResponse<Result>;
}[DeployResponseResult];

class ApplicationsController extends BaseController {
  /**
   * Get all applications in the lattice
   * @returns all of the applications in the lattice
   */
  async list() {
    const response = await this.connection.request<ApplicationListResponse | ApplicationSummary[]>(
      `${this.config.wadmTopic}.model.list`,
    );

    // TODO: See https://github.com/wasmCloud/wadm/issues/278
    // once the `list` topic correctly returns the response type, we can remove this array check
    // and the `ApplicationSummary[]` type from the union above
    if (Array.isArray(response)) {
      return {
        result: 'success',
        message: 'Successfully fetched list of models',
        models: response,
      } satisfies ApplicationListResponse;
    }

    return response;
  }

  /**
   * Convenience method to get all details for an application including status, versions, and
   * manifest. This is the same as calling `status`, `versions`, and `manifest` in parallel. If
   * any of the calls fail, the first error will be thrown.
   * @param applicationName application name to get details for
   * @returns combined details for the application
   */
  async detail(applicationName: string) {
    try {
      const [statusResponse, versionsResponse, manifestResponse] = await Promise.all([
        this.status(applicationName),
        this.versions(applicationName),
        this.manifest(applicationName),
      ]);

      return {
        status: 'ok' as const,
        message: 'Successfully fetched application details',
        detail: {
          status: statusResponse.status,
          versions: versionsResponse.versions,
          manifest: manifestResponse.manifest,
        } satisfies ApplicationDetail,
      };
    } catch {
      throw new Error('Failed to fetch application details');
    }
  }

  /**
   * Get an application by its id
   * @param applicationId the id of the application to get
   * @returns the application with the given id
   * @throws if the application is not found or if the request fails
   */
  async manifest(applicationName: string, version?: string) {
    const response = await this.connection.request<ApplicationManifestResponse>(
      `${this.config.wadmTopic}.model.get.${applicationName}`,
      version ? JSON.stringify({version}) : undefined,
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    if (response.result === 'notfound') {
      throw new Error(`Application with id ${applicationName} not found`);
    }

    return response;
  }

  /**
   * Query wadm for the history of a given application name
   * @param applicationName Name of the application to retrieve history for
   * @returns the history of the application
   */
  async versions(applicationName: string) {
    const response = await this.connection.request<ApplicationHistoryResponse>(
      `${this.config.wadmTopic}.model.versions.${applicationName}`,
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    if (response.result === 'notfound') {
      throw new Error(`Application with id ${applicationName} not found`);
    }

    return response;
  }

  /**
   * Delete a application from wadm, optionally specifying a version to delete
   * @param applicationName name of the application to delete
   * @param options.version (default: `undefined`) leaving this off will delete the latest 'put' version of the application
   * @param options.delete_all (default: `false`) if true, will delete all versions of the application
   * @returns
   */
  async delete(applicationName: string, options: {version?: string; delete_all?: boolean}) {
    const response = await this.connection.request<ApplicationDeleteResponse>(
      `${this.config.wadmTopic}.model.del.${applicationName}`,
      JSON.stringify(options),
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    if (response.result === 'noop') {
      throw new Error(`Application with id ${applicationName} not found`);
    }

    return response;
  }

  /**
   * Put an application definition, instructing wadm to store the application manifest for later deploys
   * @param yaml The full YAML or JSON string containing the OAM wadm manifest
   * @returns The response from wadm
   */
  async put(yaml: string) {
    const response = await this.connection.request<ApplicationPutResponse>(
      `${this.config.wadmTopic}.model.put`,
      yaml,
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    return response;
  }

  /**
   * Deploy an application, instructing wadm to manage the application
   * @param applicationName the name of the application to deploy. It should already exist in wadm
   * @param version The version of the application to deploy. If not provided, the latest version will be deployed
   * @returns The response from wadm
   */
  async deploy(applicationName: string, version?: string) {
    const response = await this.connection.request<ApplicationDeployResponse>(
      `${this.config.wadmTopic}.model.deploy.${applicationName}`,
      version ? JSON.stringify({version}) : undefined,
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    if (response.result === 'notfound') {
      throw new Error(`Application with id ${applicationName} not found`);
    }

    return response;
  }

  /**
   * Undeploy a application, instructing wadm to no longer manage the given application
   * @param applicationName the application name to undeploy
   * @param nonDestructive Undeploy deletes managed resources by default, this can be overridden by setting this to `true`
   * @returns The response from wadm
   */
  async undeploy(applicationName: string, nonDestructive?: boolean) {
    const response = await this.connection.request<ApplicationDeployResponse>(
      `${this.config.wadmTopic}.model.undeploy.${applicationName}`,
      JSON.stringify({non_destructive: nonDestructive ?? false}),
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    if (response.result === 'notfound') {
      throw new Error(`Application with id ${applicationName} not found`);
    }

    return response;
  }

  /**
   * Query wadm for the status of a given model by name
   * @param applicationName Name of the model to retrieve status for
   * @returns The status of the application
   */
  async status(applicationName: string) {
    const response = await this.connection.request<ApplicationStatusResponse>(
      `${this.config.wadmTopic}.model.status.${applicationName}`,
    );

    if (response.result === 'error') {
      throw new Error(response.message);
    }

    if (response.result === 'notfound') {
      throw new Error(`Application with id ${applicationName} not found`);
    }

    return response;
  }
}

export {ApplicationsController};
