import {type LatticeEvent} from './cloud-events';
import {type LatticeClient} from './lattice-client';

type LatticeObserverOptions = {
  client: LatticeClient;
};

export type LatticeSubscriber = (event: LatticeEvent) => void;

class LatticeObserver {
  readonly #client: LatticeClient;
  #subscribers: LatticeSubscriber[] = [];
  readonly #subjectSubscriptions: Array<{
    subject: string;
    unsubscribe: () => void;
  }> = [];

  constructor({client}: LatticeObserverOptions) {
    this.#client = client;
  }

  subscribe(observer: LatticeSubscriber) {
    this.#subscribers.push(observer);
    this.onSubscribe();
  }

  unsubscribe(observer: LatticeSubscriber) {
    this.#subscribers = this.#subscribers.filter((o) => o !== observer);
    this.onUnsubscribe();
  }

  notify(event: LatticeEvent) {
    for (const observer of this.#subscribers) observer(event);
  }

  onSubscribe() {
    if (this.#subscribers.length > 0) return;

    const subject = `${this.#client.instance.config.ctlTopic}.>`;
    const latticeSubscription = this.#client.instance.subscribe(subject, (event) => {
      this.notify(event);
    });
    this.#subjectSubscriptions.push({subject, unsubscribe: latticeSubscription.unsubscribe});
  }

  onUnsubscribe() {
    if (this.#subscribers.length > 0) return;

    for (const {unsubscribe} of this.#subjectSubscriptions) unsubscribe();
  }
}

export {LatticeObserver};
