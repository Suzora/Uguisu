// The login form.
import { plural } from './locale';

export const login = {
  title: 'Sign in',
  intro: 'This Uguisu needs a password. It is checked on the server and never stored in this browser.',
  user: 'User',
  password: 'Password',
  submit: 'Sign in',
  signingIn: 'Signing in…',
  retryAfter: (seconds: number) =>
    plural(seconds, { one: `Try again in ${seconds} second.`, other: `Try again in ${seconds} seconds.` }),
};
