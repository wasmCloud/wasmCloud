package wasmbus

import (
	"testing"
	"time"

	"go.wasmcloud.dev/runtime-operator/pkg/wasmbus/wasmbustest"
)

func TestNatsConnect(t *testing.T) {
	t.Run("success", func(t *testing.T) {
		defer wasmbustest.MustStartNats(t)()
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		defer nc.Close()
	})
	t.Run("error", func(t *testing.T) {
		_, err := NatsConnect(NatsDefaultURL)
		if err == nil {
			t.Fatal("expected error")
		}
	})
}

func TestNatsPublish(t *testing.T) {
	defer wasmbustest.MustStartNats(t)()
	t.Run("success", func(t *testing.T) {
		testSubject := "pubsubject"
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		defer nc.Close()
		bus := NewNatsBus(nc)

		sub, err := bus.Subscribe(testSubject, 1)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()
		received := make(chan bool)

		go sub.Handle(func(msg *Message) {
			if msg.Subject == testSubject {
				received <- true
			}
		})

		msg := NewMessage(testSubject)
		msg.Data = []byte("hello")
		err = bus.Publish(msg)
		if err != nil {
			t.Fatal(err)
		}

		if err := sub.Drain(); err != nil {
			t.Fatal(err)
		}

		select {
		case <-received:
		case <-time.After(1 * time.Second):
			t.Fatal("expected message to be received")
		}
	})
	t.Run("error", func(t *testing.T) {
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		nc.Close()
		bus := NewNatsBus(nc)
		err = bus.Publish(NewMessage("error"))
		if err == nil {
			t.Fatal("expected error")
		}
	})
}
func TestNatsRequest(t *testing.T) {}
func TestNatsSubscribe(t *testing.T) {
	defer wasmbustest.MustStartNats(t)()
	t.Run("success", func(t *testing.T) {
		testSubject := "subsubject"
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		defer nc.Close()
		bus := NewNatsBus(nc)

		sub, err := bus.Subscribe(testSubject, 1)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()

		received := make(chan bool)
		go sub.Handle(func(msg *Message) {
			if msg.Subject == testSubject {
				received <- true
			}
		})

		msg := NewMessage(testSubject)
		err = bus.Publish(msg)
		if err != nil {
			t.Fatal(err)
		}

		if err := sub.Drain(); err != nil {
			t.Fatal(err)
		}

		select {
		case <-received:
		case <-time.After(1 * time.Second):
			t.Fatal("expected message to be received")
		}
	})
	t.Run("error", func(t *testing.T) {
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		nc.Close()
		bus := NewNatsBus(nc)
		_, err = bus.Subscribe("error", NoBackLog)
		if err == nil {
			t.Fatal("expected error")
		}
	})
}

func TestNatsQueueSubscribe(t *testing.T) {
	defer wasmbustest.MustStartNats(t)()
	t.Run("success", func(t *testing.T) {
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		defer nc.Close()
		bus := NewNatsBus(nc)

		sub, err := bus.QueueSubscribe("success", "group", 1)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()
		received := make(chan bool)
		go sub.Handle(func(msg *Message) {
			if msg.Subject == "success" {
				received <- true
			}
		})

		msg := NewMessage("success")
		err = bus.Publish(msg)
		if err != nil {
			t.Fatal(err)
		}

		if err := sub.Drain(); err != nil {
			t.Fatal(err)
		}

		select {
		case <-received:
		case <-time.After(1 * time.Second):
			t.Fatal("expected message to be received")
		}
	})
	t.Run("error", func(t *testing.T) {
		nc, err := NatsConnect(NatsDefaultURL)
		if err != nil {
			t.Fatal(err)
		}
		nc.Close()
		bus := NewNatsBus(nc)
		_, err = bus.QueueSubscribe("error", "queue", NoBackLog)
		if err == nil {
			t.Fatal("expected error")
		}
	})
}
