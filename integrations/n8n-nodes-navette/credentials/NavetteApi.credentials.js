'use strict';

/**
 * navette daemon connection — where it listens, and the optional --token.
 * Everything runs on loopback; the token only exists if the daemon was
 * started with `navette serve --token SECRET`.
 */
class NavetteApi {
  constructor() {
    this.name = 'navetteApi';
    this.displayName = 'navette daemon';
    this.documentationUrl = 'https://github.com/slabbdev/navette';
    this.properties = [
      {
        displayName: 'Base URL',
        name: 'baseUrl',
        type: 'string',
        default: 'http://127.0.0.1:8765',
        placeholder: 'http://127.0.0.1:8765',
        description: 'Where the navette daemon listens (navette serve --port …)',
      },
      {
        displayName: 'Token',
        name: 'token',
        type: 'string',
        typeOptions: { password: true },
        default: '',
        description: 'Only if the daemon runs with --token',
      },
    ];
    // Loopback daemons are personal — same credential is fine for all ops.
    this.authenticate = {
      type: 'generic',
      properties: {},
    };
  }
}

module.exports = { NavetteApi };
