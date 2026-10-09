import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import Login from './Login.svelte';
import { setCsrfToken } from '../lib/api';
import { stubApi } from '../tests/harness';

const SESSION = {
  body: { username: 'uguisu', csrf_token: 'csrf-1', expires_at: '2026-02-01T00:00:00Z', schema: 1 },
};

async function fillIn(password: string): Promise<void> {
  await userEvent.type(screen.getByLabelText('Password'), password);
  await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));
}

describe('login view', () => {
  it('seeds the user the server named', () => {
    stubApi({});
    render(Login, { props: { onauthenticated: () => {}, username: 'operator' } });

    expect(screen.getByLabelText('User')).toHaveValue('operator');
  });

  it('reports a session once the password is accepted', async () => {
    const api = stubApi({ 'POST /api/v1/auth/login': SESSION });
    const authenticated = vi.fn();
    render(Login, { props: { onauthenticated: authenticated } });

    await fillIn('correct horse');

    await waitFor(() => expect(authenticated).toHaveBeenCalledOnce());
    expect(api.calls.at(-1)?.body).toEqual({ username: 'uguisu', password: 'correct horse' });
  });

  it('keeps the password out of the URL', async () => {
    const api = stubApi({ 'POST /api/v1/auth/login': SESSION });
    render(Login, { props: { onauthenticated: () => {} } });

    await fillIn('correct horse');

    await waitFor(() => expect(api.calls.length).toBeGreaterThan(0));
    for (const call of api.calls) {
      expect(call.path).not.toContain('correct');
    }
  });

  it('shows the server message for a wrong password', async () => {
    stubApi({
      'POST /api/v1/auth/login': {
        status: 401,
        body: { error: { kind: 'unauthenticated', message: 'username or password is incorrect' } },
      },
    });
    render(Login, { props: { onauthenticated: () => {} } });

    await fillIn('wrong');

    expect(await screen.findByRole('alert')).toHaveTextContent('username or password is incorrect');
  });

  it('says how long to wait when refused for too many tries', async () => {
    stubApi({
      'POST /api/v1/auth/login': {
        status: 429,
        headers: { 'retry-after': '300' },
        body: { error: { kind: 'too_many_requests', message: 'too many failed logins' } },
      },
    });
    render(Login, { props: { onauthenticated: () => {} } });

    await fillIn('wrong');

    expect(await screen.findByRole('alert')).toHaveTextContent('Try again in 300 seconds.');
  });

  it('sends the csrf header the session handed out', async () => {
    setCsrfToken(null);
    const api = stubApi({ 'POST /api/v1/auth/login': SESSION, 'POST /api/v1/podcasts': { body: {} } });
    render(Login, { props: { onauthenticated: () => {} } });

    await fillIn('correct horse');
    await waitFor(() => expect(api.calls.length).toBeGreaterThan(0));

    const { addPodcast } = await import('../lib/api');
    await addPodcast('https://feeds.example/x.xml');
    const added = api.calls.find((call) => call.path.endsWith('/api/v1/podcasts'));
    expect(added?.headers['x-uguisu-csrf']).toBe('csrf-1');
  });
});
