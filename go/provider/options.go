package provider

type ProviderHandler func(*WasmcloudProvider) error

func SourceLinkPut(inFunc func(InterfaceLinkDefinition) error) ProviderHandler {
	return func(wp *WasmcloudProvider) error {
		wp.putSourceLinkFunc = inFunc
		return nil
	}
}

func TargetLinkPut(inFunc func(InterfaceLinkDefinition) error) ProviderHandler {
	return func(wp *WasmcloudProvider) error {
		wp.putTargetLinkFunc = inFunc
		return nil
	}
}

func SourceLinkDel(inFunc func(InterfaceLinkDefinition) error) ProviderHandler {
	return func(wp *WasmcloudProvider) error {
		wp.delSourceLinkFunc = inFunc
		return nil
	}
}

func TargetLinkDel(inFunc func(InterfaceLinkDefinition) error) ProviderHandler {
	return func(wp *WasmcloudProvider) error {
		wp.delTargetLinkFunc = inFunc
		return nil
	}
}

func Shutdown(inFunc func() error) ProviderHandler {
	return func(wp *WasmcloudProvider) error {
		wp.shutdownFunc = inFunc
		return nil
	}
}

func HealthCheck(inFunc func() string) ProviderHandler {
	return func(wp *WasmcloudProvider) error {
		wp.healthMsgFunc = inFunc
		return nil
	}
}
