package provider

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"log"
	"log/slog"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	wrpcnats "github.com/bytecodealliance/wrpc/go/nats"
	nats "github.com/nats-io/nats.go"
	"github.com/nats-io/nkeys"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/log/global"
)

type WasmcloudProvider struct {
	Id string

	context context.Context
	cancel  context.CancelFunc
	Logger  *slog.Logger

	hostData     HostData
	hostXkey     nkeys.KeyPair
	providerXkey nkeys.KeyPair

	Topics Topics

	RPCClient *wrpcnats.Client

	natsConnection    *nats.Conn
	natsSubscriptions map[string]*nats.Subscription

	healthMsgFunc func() string

	shutdownFunc func() error
	// internalShutdownFuncs holds a list of callbacks triggered during shutdown (ex: opentelemetry exporter graceful shutdown).
	// They are called after the user provided `shutdownFunc` and nats disconnect.
	internalShutdownFuncs []func(context.Context) error
	shutdown              chan struct{}

	putSourceLinkFunc func(InterfaceLinkDefinition) error
	putTargetLinkFunc func(InterfaceLinkDefinition) error
	delSourceLinkFunc func(InterfaceLinkDefinition) error
	delTargetLinkFunc func(InterfaceLinkDefinition) error

	lock sync.Mutex
	// Links from the provider to other components, aka where the provider is the
	// source of the link. Indexed by the component ID of the target
	sourceLinks map[string]InterfaceLinkDefinition
	// Links from other components to the provider, aka where the provider is the
	// target of the link. Indexed by the component ID of the source
	targetLinks map[string]InterfaceLinkDefinition
}

func New(options ...ProviderHandler) (*WasmcloudProvider, error) {
	reader := bufio.NewReader(os.Stdin)

	// Make a channel to receive the host data so we can timeout if we don't receive it
	// All host data is sent immediately after the provider starts
	hostDataChannel := make(chan string, 1)
	go func() {
		hostDataRaw, err := reader.ReadString('\n')
		if err != nil {
			log.Fatal(err)
		}
		hostDataChannel <- hostDataRaw
	}()

	hostData := HostData{}
	select {
	case hostDataRaw := <-hostDataChannel:
		decodedData, err := base64.StdEncoding.DecodeString(hostDataRaw)
		if err != nil {
			return nil, err
		}

		err = json.Unmarshal(decodedData, &hostData)
		if err != nil {
			return nil, err
		}
	case <-time.After(5 * time.Second):
		log.Fatal("failed to read host data, did not receive after 5 seconds")
	}

	// Initialize Logging
	var logger *slog.Logger
	var level Level
	if hostData.LogLevel != nil {
		level = *hostData.LogLevel
	} else {
		level = Info
	}
	if hostData.StructuredLogging {
		logger = slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{Level: level}))
	} else {
		logger = slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: level}))
	}

	var internalShutdownFuncs []func(context.Context) error

	// Initialize Observability
	propagator := newPropagator()
	otel.SetTextMapPropagator(propagator)

	serviceResource, err := newServiceResource(context.Background(), hostData.ProviderKey)
	if err != nil {
		return nil, err
	}

	if hostData.OtelConfig.EnableObservability || (hostData.OtelConfig.EnableMetrics != nil && *hostData.OtelConfig.EnableMetrics) {
		meterProvider, err := newMeterProvider(context.Background(), hostData.OtelConfig, serviceResource)
		if err != nil {
			return nil, err
		}
		otel.SetMeterProvider(meterProvider)
		internalShutdownFuncs = append(internalShutdownFuncs, func(c context.Context) error { return meterProvider.Shutdown(c) })
	}

	if hostData.OtelConfig.EnableObservability || (hostData.OtelConfig.EnableTraces != nil && *hostData.OtelConfig.EnableTraces) {
		tracerProvider, err := newTracerProvider(context.Background(), hostData.OtelConfig, serviceResource)
		if err != nil {
			return nil, err
		}
		otel.SetTracerProvider(tracerProvider)
		internalShutdownFuncs = append(internalShutdownFuncs, func(c context.Context) error { return tracerProvider.Shutdown(c) })
	}

	if hostData.OtelConfig.EnableObservability || (hostData.OtelConfig.EnableLogs != nil && *hostData.OtelConfig.EnableLogs) {
		loggerProvider, err := newLoggerProvider(context.Background(), hostData.OtelConfig, serviceResource)
		if err != nil {
			return nil, err
		}
		global.SetLoggerProvider(loggerProvider)
		internalShutdownFuncs = append(internalShutdownFuncs, func(c context.Context) error { return loggerProvider.Shutdown(c) })
	}

	// Connect to NATS
	nc, err := nats.Connect(hostData.LatticeRPCURL)
	if err != nil {
		return nil, err
	}

	logger.Debug("host config", "config", hostData)

	var hostXkey nkeys.KeyPair
	if len(hostData.HostXKeyPublicKey) == 0 {
		// If the host xkey is not provided, secrets won't be sent to the provider
		// so we can just create a new xkey
		hostXkey, err = nkeys.CreateCurveKeys()
		if err != nil {
			return nil, err
		}
	} else {
		hostXkey, err = nkeys.FromPublicKey(hostData.HostXKeyPublicKey)
		if err != nil {
			logger.Error("failed to create host xkey from public key", slog.Any("error", err))
			return nil, err
		}
	}

	var providerXkey nkeys.KeyPair
	if len(hostData.ProviderXKeyPrivateKey.Reveal()) == 0 {
		// If the provider xkey is not provided, secrets won't be sent to the provider
		// so we can just create a new xkey
		providerXkey, err = nkeys.CreateCurveKeys()
		if err != nil {
			return nil, err
		}
	} else {
		providerXkey, err = nkeys.FromCurveSeed([]byte(hostData.ProviderXKeyPrivateKey.Reveal()))
		if err != nil {
			logger.Error("failed to create provider xkey from private key", slog.Any("error", err))
			return nil, err
		}
	}

	// partition links based on if the provider is the source or target
	sourceLinks := []linkWithEncryptedSecrets{}
	targetLinks := []linkWithEncryptedSecrets{}

	// Loop over the numbers
	for _, link := range hostData.LinkDefinitions {
		if link.SourceID == hostData.ProviderKey {
			sourceLinks = append(sourceLinks, link)
		} else if link.Target == hostData.ProviderKey {
			targetLinks = append(targetLinks, link)
		} else {
			logger.Warn("Link %s->%s is not connected to provider, ignoring", link.SourceID, link.Target)
		}
	}

	prefix := fmt.Sprintf("%s.%s", hostData.LatticeRPCPrefix, hostData.ProviderKey)
	wrpc := wrpcnats.NewClientWithQueueGroup(nc, prefix, prefix)

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)

	ctx, cancel := context.WithCancel(context.Background())
	provider := &WasmcloudProvider{
		Id:        hostData.ProviderKey,
		Logger:    logger,
		RPCClient: wrpc,
		Topics:    LatticeTopics(hostData, providerXkey),

		context: ctx,
		cancel:  cancel,

		hostData:     hostData,
		hostXkey:     hostXkey,
		providerXkey: providerXkey,

		natsConnection:    nc,
		natsSubscriptions: map[string]*nats.Subscription{},

		healthMsgFunc: func() string { return "healthy" },

		shutdownFunc:          func() error { return nil },
		internalShutdownFuncs: internalShutdownFuncs,
		shutdown:              make(chan struct{}),

		putSourceLinkFunc: func(InterfaceLinkDefinition) error { return nil },
		putTargetLinkFunc: func(InterfaceLinkDefinition) error { return nil },
		delSourceLinkFunc: func(InterfaceLinkDefinition) error { return nil },
		delTargetLinkFunc: func(InterfaceLinkDefinition) error { return nil },

		sourceLinks: make(map[string]InterfaceLinkDefinition, len(sourceLinks)),
		targetLinks: make(map[string]InterfaceLinkDefinition, len(targetLinks)),
	}

	for _, opt := range options {
		err := opt(provider)
		if err != nil {
			return nil, err
		}
	}

	for _, link := range sourceLinks {
		decryptedLink, err := provider.DecryptLinkSecrets(link)
		if err != nil {
			logger.Error("failed to decrypt secrets on link", slog.Any("error", err))
		}
		err = provider.updateProviderLinkMap(decryptedLink)
		if err != nil {
			logger.Error("failed to update provider link map", slog.Any("error", err))
		}
	}

	for _, link := range targetLinks {
		decryptedLink, err := provider.DecryptLinkSecrets(link)
		if err != nil {
			logger.Error("failed to decrypt secrets on link", slog.Any("error", err))
		}
		err = provider.updateProviderLinkMap(decryptedLink)
		if err != nil {
			logger.Error("failed to update provider link map", slog.Any("error", err))
		}
	}
	return provider, nil
}

func (wp *WasmcloudProvider) HostData() HostData {
	return wp.hostData
}

func (wp *WasmcloudProvider) NatsConnection() *nats.Conn {
	return wp.natsConnection
}

func (wp *WasmcloudProvider) OutgoingRpcClient(target string) *wrpcnats.Client {
	return wrpcnats.NewClient(wp.natsConnection, fmt.Sprintf("%s.%s", wp.hostData.LatticeRPCPrefix, target))
}

func (wp *WasmcloudProvider) Start() error {
	for _, link := range wp.sourceLinks {
		err := wp.putSourceLinkFunc(link)
		if err != nil {
			wp.Logger.Error("failed to invoke source link function", slog.Any("error", err))
		}
	}
	for _, link := range wp.targetLinks {
		err := wp.putTargetLinkFunc(link)
		if err != nil {
			wp.Logger.Error("failed to invoke target link function", slog.Any("error", err))
		}
	}

	err := wp.subToNats()
	if err != nil {
		return err
	}

	wp.Logger.Info("provider started", "id", wp.Id)
	<-wp.context.Done()
	wp.Logger.Info("provider exiting", "id", wp.Id)
	return nil
}

func (wp *WasmcloudProvider) Shutdown() error {
	err := wp.shutdownFunc()
	if err != nil {
		wp.cancel()
		return err
	}

	err = wp.cleanupNatsSubscriptions()
	if err != nil {
		wp.cancel()
		return err
	}

	for _, errFunc := range wp.internalShutdownFuncs {
		if err := errFunc(wp.context); err != nil {
			wp.cancel()
			return err
		}
	}

	wp.cancel()
	return nil
}

func (wp *WasmcloudProvider) subToNats() error {
	// ------------------ Subscribe to Health topic --------------------
	health, err := wp.natsConnection.Subscribe(wp.Topics.LATTICE_HEALTH,
		func(m *nats.Msg) {
			msg := wp.healthMsgFunc()
			hc := HealthCheckResponse{
				Healthy: true,
				Message: msg,
			}

			hcBytes, err := json.Marshal(hc)
			if err != nil {
				wp.Logger.Error("failed to encode health check", slog.Any("error", err))
				return
			}

			err = wp.natsConnection.Publish(m.Reply, hcBytes)
			if err != nil {
				wp.Logger.Error("failed to publish health check response", slog.Any("error", err))
			}
		})
	if err != nil {
		wp.Logger.Error("LATTICE_HEALTH", slog.Any("error", err))
		return err
	}

	wp.natsSubscriptions[wp.Topics.LATTICE_HEALTH] = health

	// ------------------ Subscribe to Delete link topic --------------
	linkDel, err := wp.natsConnection.Subscribe(wp.Topics.LATTICE_LINK_DEL,
		func(m *nats.Msg) {
			link := InterfaceLinkDefinition{}
			err := json.Unmarshal(m.Data, &link)
			if err != nil {
				wp.Logger.Error("failed to decode link", slog.Any("error", err))
				return
			}

			err = wp.deleteLink(link)
			if err != nil {
				// TODO(#10): handle better?
				wp.Logger.Error("failed to delete link", slog.Any("error", err))
				return
			}
		})
	if err != nil {
		wp.Logger.Error("LINK_DEL", slog.Any("error", err))
		return err
	}

	wp.natsSubscriptions[wp.Topics.LATTICE_LINK_DEL] = linkDel

	// ------------------ Subscribe to New link topic --------------
	linkPut, err := wp.natsConnection.Subscribe(wp.Topics.LATTICE_LINK_PUT,
		func(m *nats.Msg) {
			link := linkWithEncryptedSecrets{}
			err := json.Unmarshal(m.Data, &link)
			if err != nil {
				wp.Logger.Error("failed to decode link", slog.Any("error", err))
				return
			}

			providerLink, err := wp.DecryptLinkSecrets(link)
			if err != nil {
				wp.Logger.Error("failed to decrypt secrets on link", slog.Any("error", err))
				return
			}

			err = wp.putLink(providerLink)
			if err != nil {
				// TODO(#10): handle this better?
				wp.Logger.Error("newLinkFunc", slog.Any("error", err))
			}
		})
	if err != nil {
		wp.Logger.Error("LINK_PUT", slog.Any("error", err))
		return err
	}

	wp.natsSubscriptions[wp.Topics.LATTICE_LINK_PUT] = linkPut

	// ------------------ Subscribe to Shutdown topic ------------------
	shutdown, err := wp.natsConnection.Subscribe(wp.Topics.LATTICE_SHUTDOWN,
		func(m *nats.Msg) {
			err := wp.shutdownFunc()
			if err != nil {
				// TODO(#10): handle this better?
				wp.Logger.Error("ERROR: provider shutdown function failed: " + err.Error())
			}

			err = m.Respond([]byte("provider shutdown handled successfully"))
			if err != nil {
				// NOTE: This is a log message because we don't want to stop the shutdown process
				wp.Logger.Error("ERROR: provider shutdown failed to respond: " + err.Error())
			}

			err = wp.cleanupNatsSubscriptions()
			if err != nil {
				wp.Logger.Error("ERROR: provider shutdown failed to drain connection: " + err.Error())
			}

			wp.cancel()
		})
	if err != nil {
		wp.Logger.Error("LATTICE_SHUTDOWN", slog.Any("error", err))
		return err
	}

	wp.natsSubscriptions[wp.Topics.LATTICE_SHUTDOWN] = shutdown
	return nil
}

func (wp *WasmcloudProvider) cleanupNatsSubscriptions() error {
	err := wp.natsConnection.Flush()
	if err != nil {
		return err
	}

	for _, s := range wp.natsSubscriptions {
		err := s.Drain()
		if err != nil {
			// NOTE: This is a log message because we don't want to stop the shutdown process
			wp.Logger.Error("ERROR: provider shutdown failed to drain subscription: " + err.Error())
		}
	}

	return wp.natsConnection.Drain()
}

func (wp *WasmcloudProvider) DecryptLinkSecrets(h linkWithEncryptedSecrets) (InterfaceLinkDefinition, error) {
	sourceSecrets, err := DecryptSecrets(h.SourceSecrets, wp.providerXkey, wp.hostData.HostXKeyPublicKey)
	if err != nil {
		return InterfaceLinkDefinition{}, err
	}

	targetSecrets, err := DecryptSecrets(h.TargetSecrets, wp.providerXkey, wp.hostData.HostXKeyPublicKey)
	if err != nil {
		return InterfaceLinkDefinition{}, err
	}

	return InterfaceLinkDefinition{
		SourceID:      h.SourceID,
		Target:        h.Target,
		Name:          h.Name,
		WitNamespace:  h.WitNamespace,
		WitPackage:    h.WitPackage,
		Interfaces:    h.Interfaces,
		SourceConfig:  h.SourceConfig,
		TargetConfig:  h.TargetConfig,
		SourceSecrets: sourceSecrets,
		TargetSecrets: targetSecrets,
	}, nil
}

func (wp *WasmcloudProvider) putLink(l InterfaceLinkDefinition) error {
	// Ignore duplicate links
	if wp.isLinked(l.SourceID, l.Target) {
		wp.Logger.Info("ignoring duplicate link", "link", l)
		return nil
	}

	wp.lock.Lock()
	defer wp.lock.Unlock()
	if l.SourceID == wp.Id {
		err := wp.putSourceLinkFunc(l)
		if err != nil {
			return err
		}

		wp.sourceLinks[l.Target] = l
	} else if l.Target == wp.Id {
		err := wp.putTargetLinkFunc(l)
		if err != nil {
			return err
		}

		wp.targetLinks[l.SourceID] = l
	} else {
		wp.Logger.Info("received link that isn't for this provider, ignoring", "link", l)
	}
	return nil
}

func (wp *WasmcloudProvider) updateProviderLinkMap(l InterfaceLinkDefinition) error {
	// Ignore duplicate links
	if wp.isLinked(l.SourceID, l.Target) {
		wp.Logger.Info("ignoring duplicate link", "link", l)
		return nil
	}
	wp.lock.Lock()
	defer wp.lock.Unlock()
	if l.SourceID == wp.Id {
		wp.sourceLinks[l.Target] = l
	} else if l.Target == wp.Id {
		wp.targetLinks[l.SourceID] = l
	} else {
		wp.Logger.Info("received link that isn't for this provider, ignoring", "link", l)
	}
	return nil
}

func (wp *WasmcloudProvider) deleteLink(l InterfaceLinkDefinition) error {
	wp.lock.Lock()
	defer wp.lock.Unlock()
	if l.SourceID == wp.Id {
		err := wp.delSourceLinkFunc(l)
		if err != nil {
			return err
		}

		delete(wp.sourceLinks, l.Target)
	} else if l.Target == wp.Id {
		err := wp.delTargetLinkFunc(l)
		if err != nil {
			return err
		}

		delete(wp.targetLinks, l.SourceID)
	} else {
		wp.Logger.Info("received link delete that isn't for this provider, ignoring", "link", l)
	}

	return nil
}

func (wp *WasmcloudProvider) isLinked(sourceId string, target string) bool {
	wp.lock.Lock()
	defer wp.lock.Unlock()
	if sourceId == wp.Id {
		_, exists := wp.sourceLinks[target]
		return exists
	} else if target == wp.Id {
		_, exists := wp.targetLinks[sourceId]
		return exists
	} else {
		return false
	}
}
