'use strict';

/**
 * LLM configuration for the navette agent operation — any OpenAI-compatible
 * endpoint. Defaults to Google Gemini's free OpenAI-compat layer; works the
 * same with Z.ai (https://api.z.ai/api/paas/v4), OpenAI, Ollama, anything.
 */
class NavetteLlmApi {
  constructor() {
    this.name = 'navetteLlmApi';
    this.displayName = 'navette agent — LLM';
    this.documentationUrl = 'https://github.com/slabbdev/navette';
    this.properties = [
      {
        displayName: 'LLM Base URL',
        name: 'baseUrl',
        type: 'string',
        default: 'https://generativelanguage.googleapis.com/v1beta/openai',
        placeholder: 'https://generativelanguage.googleapis.com/v1beta/openai',
        description: 'OpenAI-compatible endpoint (Gemini free tier by default; Z.ai: https://api.z.ai/api/paas/v4)',
      },
      {
        displayName: 'API Key',
        name: 'apiKey',
        type: 'string',
        typeOptions: { password: true },
        default: '',
        description: 'The endpoint API key',
      },
      {
        displayName: 'Model',
        name: 'model',
        type: 'string',
        default: 'gemini-2.5-flash',
        description: 'Model id, e.g. gemini-2.5-flash, glm-4.6, gpt-4o-mini',
      },
    ];
  }
}

module.exports = { NavetteLlmApi };
