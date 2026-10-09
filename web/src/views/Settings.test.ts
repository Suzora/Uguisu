import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import Settings from './Settings.svelte';
import { EMPTY, stubApi } from '../tests/harness';

function key(overrides: Record<string, unknown> = {}) {
  return {
    key: 'UGUISU_FEED_REFRESH_INTERVAL_SECS',
    value: '3600',
    origin: 'default',
    stored: null,
    pinned: false,
    persistable: true,
    live: true,
    ...overrides,
  };
}

function report(keys: unknown[], extra: Record<string, unknown> = {}) {
  return { body: { keys, rejected: [], unused: [], schema: 1, ...extra } };
}

describe('settings view', () => {
  it('offers an editable value', async () => {
    stubApi({ '/api/v1/settings': report([key()]) });
    render(Settings, { props: { query: {} } });

    const input = await screen.findByLabelText('Value of UGUISU_FEED_REFRESH_INTERVAL_SECS');
    expect(input).toBeEnabled();
    expect(input).toHaveValue('3600');
    expect(screen.getByText('default')).toBeInTheDocument();
  });

  it('shows no control for an environment value', async () => {
    // `pinned` is false here: the API sets it only when a stored value also
    // exists, but a write would still be refused with a conflict.
    stubApi({
      '/api/v1/settings': report([key({ origin: 'env', pinned: false, stored: null, value: '1800' })]),
    });
    render(Settings, { props: { query: {} } });

    expect(await screen.findByText('pinned by the environment')).toBeInTheDocument();
    expect(screen.getByText(/In force from the environment/)).toBeInTheDocument();
    expect(
      screen.queryByLabelText('Value of UGUISU_FEED_REFRESH_INTERVAL_SECS'),
    ).not.toBeInTheDocument();
  });

  it('names a stored value the environment overrides', async () => {
    stubApi({
      '/api/v1/settings': report([key({ origin: 'env', pinned: true, stored: '900', value: '1800' })]),
    });
    render(Settings, { props: { query: {} } });

    expect(await screen.findByText(/is being kept but ignored/)).toBeInTheDocument();
    expect(screen.getByText('900')).toBeInTheDocument();
  });

  it('shows no control for a command-line value', async () => {
    stubApi({ '/api/v1/settings': report([key({ origin: 'cli', value: '1800' })]) });
    render(Settings, { props: { query: {} } });

    expect(await screen.findByText('pinned by a command-line flag')).toBeInTheDocument();
    expect(
      screen.queryByLabelText('Value of UGUISU_FEED_REFRESH_INTERVAL_SECS'),
    ).not.toBeInTheDocument();
  });

  it('marks a key that needs a restart', async () => {
    stubApi({ '/api/v1/settings': report([key({ live: false })]) });
    render(Settings, { props: { query: {} } });
    expect(await screen.findByText('restart required')).toBeInTheDocument();
  });

  it('keeps a refused stored value visible', async () => {
    stubApi({
      '/api/v1/settings': report([key({ stored: 'banana', value: '3600' })], {
        rejected: [
          {
            key: 'UGUISU_FEED_REFRESH_INTERVAL_SECS',
            value: 'banana',
            message: 'expected a number of seconds',
          },
        ],
      }),
    });
    render(Settings, { props: { query: {} } });

    expect(await screen.findByText('Stored values that were refused')).toBeInTheDocument();
    expect(screen.getByText('expected a number of seconds')).toBeInTheDocument();
    expect(screen.getByText(/but/)).toBeInTheDocument();
  });

  it('surfaces a rejected write', async () => {
    stubApi({
      '/api/v1/settings': report([key()]),
      'PUT /api/v1/settings/UGUISU_FEED_REFRESH_INTERVAL_SECS': {
        status: 400,
        body: {
          error: {
            kind: 'invalid',
            message: 'UGUISU_FEED_REFRESH_INTERVAL_SECS: expected a number of seconds',
          },
        },
      },
    });
    const { container } = render(Settings, { props: { query: {} } });

    const input = (await screen.findByLabelText(
      'Value of UGUISU_FEED_REFRESH_INTERVAL_SECS',
    )) as HTMLInputElement;
    input.value = 'banana';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled());
    screen.getByRole('button', { name: 'Save' }).click();

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('expected a number of seconds'),
    );
    expect(container.querySelector('input')).toBeInTheDocument();
  });

  it('explains a write the environment blocks', async () => {
    stubApi({
      '/api/v1/settings': report([key({ stored: null })]),
      'PUT /api/v1/settings/UGUISU_FEED_REFRESH_INTERVAL_SECS': {
        status: 409,
        body: {
          error: {
            kind: 'conflict',
            message:
              'UGUISU_FEED_REFRESH_INTERVAL_SECS is set in the environment (env) and a stored value would be ignored; unset the environment variable to manage it here',
          },
        },
      },
    });
    render(Settings, { props: { query: {} } });

    const input = (await screen.findByLabelText(
      'Value of UGUISU_FEED_REFRESH_INTERVAL_SECS',
    )) as HTMLInputElement;
    input.value = '900';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled());
    screen.getByRole('button', { name: 'Save' }).click();

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('unset the environment variable'),
    );
  });

  it('lists a stored value nothing reads', async () => {
    stubApi({
      '/api/v1/settings': report([key()], {
        unused: [{ key: 'UGUISU_OLD_KEY', value: 'x', reason: 'unknown' }],
      }),
    });
    render(Settings, { props: { query: {} } });
    expect(await screen.findByText('Stored values that are ignored')).toBeInTheDocument();
    expect(screen.getByText('UGUISU_OLD_KEY')).toBeInTheDocument();
  });
});

const NO_TOKENS = { body: { tokens: [], schema: 1 } };

describe('settings password panel', () => {
  const SIGNED_IN = {
    ...EMPTY.session,
    auth_required: true,
    authenticated: true,
    credential_set: true,
    username: 'uguisu',
  };

  async function submit(next: string, repeated: string, button: string): Promise<void> {
    await userEvent.type(await screen.findByLabelText('New password'), next);
    await userEvent.type(screen.getByLabelText('Repeat it'), repeated);
    await userEvent.click(screen.getByRole('button', { name: button }));
  }

  it('sets the first password', async () => {
    const api = stubApi({
      '/api/v1/settings': report([]),
      '/api/v1/auth/session': [{ body: EMPTY.session }, { body: SIGNED_IN }],
      '/api/v1/auth/tokens': NO_TOKENS,
      'POST /api/v1/auth/password': { status: 204 },
    });
    render(Settings, { props: { query: {} } });

    await submit('correct horse', 'correct horse', 'Set password');

    expect(await screen.findByText('Password set. Every other session has been signed out.')).toBeInTheDocument();
    expect(api.calls.find((c) => c.method === 'POST')?.body).toEqual({ new_password: 'correct horse' });
    expect(await screen.findByRole('heading', { name: 'Password' })).toBeInTheDocument();
  });

  it('refuses two different passwords', async () => {
    const api = stubApi({ '/api/v1/settings': report([]) });
    render(Settings, { props: { query: {} } });

    await submit('correct horse', 'battery staple', 'Set password');

    expect(await screen.findByRole('alert')).toHaveTextContent('The two new passwords are not the same.');
    expect(api.calls.filter((c) => c.method === 'POST')).toEqual([]);
  });

  it('asks for the current password', async () => {
    const api = stubApi({
      '/api/v1/settings': report([]),
      '/api/v1/auth/session': { body: SIGNED_IN },
      '/api/v1/auth/tokens': NO_TOKENS,
      'POST /api/v1/auth/password': {
        status: 403,
        body: { error: { kind: 'forbidden', message: 'the current password is required to change it' } },
      },
    });
    render(Settings, { props: { query: {} } });

    await userEvent.type(await screen.findByLabelText('Current password'), 'wrong');
    await submit('correct horse', 'correct horse', 'Change password');

    expect(await screen.findByRole('alert')).toHaveTextContent('the current password is required to change it');
    expect(api.calls.find((c) => c.method === 'POST')?.body).toEqual({
      current_password: 'wrong',
      new_password: 'correct horse',
    });
  });
});

describe('settings token panel', () => {
  const SIGNED_IN = {
    ...EMPTY.session,
    auth_required: true,
    authenticated: true,
    credential_set: true,
    username: 'uguisu',
  };
  const TOKEN = {
    id: '01J0000000000000000000000T',
    name: 'backup script',
    scope: 'read',
    created_at: '2026-01-01T10:00:00Z',
    last_used_at: null,
    expires_at: null,
    revoked_at: null,
  };

  it('needs a password first', async () => {
    const api = stubApi({ '/api/v1/settings': report([]) });
    render(Settings, { props: { query: {} } });

    expect(await screen.findByRole('heading', { name: 'Set a password' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'API tokens' })).not.toBeInTheDocument();
    expect(api.calls.some((c) => c.path.startsWith('/api/v1/auth/tokens'))).toBe(false);
  });

  it('shows a new secret once', async () => {
    const api = stubApi({
      '/api/v1/settings': report([]),
      '/api/v1/auth/session': { body: SIGNED_IN },
      '/api/v1/auth/tokens': [NO_TOKENS, { body: { tokens: [TOKEN], schema: 1 } }],
      'POST /api/v1/auth/tokens': {
        status: 201,
        body: { token: TOKEN, secret: 'ugt_secret_value', schema: 1 },
      },
    });
    render(Settings, { props: { query: {} } });

    expect(await screen.findByText('No token yet.')).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText('Name'), 'backup script');
    await userEvent.click(screen.getByRole('button', { name: 'Create token' }));

    expect(await screen.findByLabelText('The new token')).toHaveValue('ugt_secret_value');
    expect(
      screen.getByText('Created “backup script”. Copy the token now: it is not shown again.'),
    ).toBeInTheDocument();
    expect(api.calls.find((c) => c.method === 'POST')?.body).toEqual({
      name: 'backup script',
      scope: 'read',
    });
    expect(await screen.findByRole('cell', { name: 'backup script' })).toBeInTheDocument();
    expect(api.calls.every((c) => !c.path.includes('ugt_secret_value'))).toBe(true);
  });

  it('revokes after asking', async () => {
    const api = stubApi({
      '/api/v1/settings': report([]),
      '/api/v1/auth/session': { body: SIGNED_IN },
      '/api/v1/auth/tokens': [
        { body: { tokens: [TOKEN], schema: 1 } },
        { body: { tokens: [{ ...TOKEN, revoked_at: '2026-01-02T10:00:00Z' }], schema: 1 } },
      ],
      [`DELETE /api/v1/auth/tokens/${TOKEN.id}`]: { status: 204 },
    });
    render(Settings, { props: { query: {} } });

    await userEvent.click(await screen.findByRole('button', { name: 'Revoke' }));
    expect(
      screen.getByText('Revoke “backup script”? Anything that uses it stops working.'),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Revoke' }));

    expect(await screen.findByText('Revoked “backup script”.')).toBeInTheDocument();
    expect(api.calls.some((c) => c.method === 'DELETE')).toBe(true);
    expect(await screen.findByText('revoked')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Revoke' })).not.toBeInTheDocument();
  });
});

