import { connect } from 'nats.ws';

export async function canConnect(url: string): Promise<boolean> {
  try {
    const connection = await connect({ servers: url });
    await connection.close();
    return true;
  } catch {
    return false;
  }
}
