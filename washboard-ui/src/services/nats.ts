import {connect, Authenticator} from 'nats.ws';

export async function canConnect(args: {
  servers: Array<string> | string;
  authenticator?: Authenticator[];
}): Promise<boolean> {
  try {
    const connection = await connect(args);
    await connection.close();
    return true;
  } catch {
    return false;
  }
}
