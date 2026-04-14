import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://clement-tourriere.github.io',
  base: '/dbcrust',
  integrations: [
    starlight({
      title: 'DBCrust',
      favicon: '/favicon.svg',
      customCss: ['./src/styles/custom.css'],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/clement-tourriere/dbcrust' },
      ],
      editLink: {
        baseUrl: 'https://github.com/clement-tourriere/dbcrust/edit/main/docs/src/content/docs/',
      },
      expressiveCode: {
        themes: ['github-dark', 'github-light'],
      },
      sidebar: [
        {
          label: 'Getting Started',
          items: [
            { label: 'Quick Start', slug: 'quick-start' },
            { label: 'Installation', slug: 'installation' },
            { label: 'Configuration', slug: 'configuration' },
          ],
        },
        {
          label: 'User Guide',
          items: [
            { label: 'Basic Usage', slug: 'user-guide/basic-usage' },
            { label: 'Desktop GUI', slug: 'user-guide/gui' },
            { label: 'Advanced Features', slug: 'user-guide/advanced-features' },
            { label: 'Performance Analysis', slug: 'user-guide/performance-analysis' },
            { label: 'File Formats', slug: 'user-guide/file-formats' },
            { label: 'MongoDB', slug: 'user-guide/mongodb' },
            { label: 'Elasticsearch', slug: 'user-guide/elasticsearch' },
            { label: 'Password Management', slug: 'user-guide/password-management' },
            { label: 'Troubleshooting', slug: 'user-guide/troubleshooting' },
            { label: 'Development', slug: 'user-guide/development' },
          ],
        },
        {
          label: 'Django Integration',
          items: [
            { label: 'ORM Analyzer', slug: 'django-analyzer' },
            { label: 'Middleware', slug: 'django/middleware' },
            { label: 'Management Commands', slug: 'django/management-commands' },
            { label: 'CI/CD Integration', slug: 'django/ci-integration' },
            { label: 'Team Workflows', slug: 'django/team-workflows' },
          ],
        },
        {
          label: 'Python API',
          items: [
            { label: 'Overview', slug: 'python-api/overview' },
            { label: 'Direct Execution', slug: 'python-api/direct-execution' },
            { label: 'Client Classes', slug: 'python-api/client-classes' },
            { label: 'Django Integration', slug: 'python-api/django-integration' },
            { label: 'Error Handling', slug: 'python-api/error-handling' },
            { label: 'Examples', slug: 'python-api/examples' },
          ],
        },
        {
          label: 'Advanced',
          items: [
            { label: 'SSH Tunneling', slug: 'advanced/ssh-tunneling' },
            { label: 'Vault Integration', slug: 'advanced/vault-integration' },
            { label: 'Docker Integration', slug: 'advanced/docker-integration' },
            { label: 'Security', slug: 'advanced/security' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Backslash Commands', slug: 'reference/backslash-commands' },
            { label: 'URL Schemes', slug: 'reference/url-schemes' },
            { label: 'Configuration Reference', slug: 'reference/configuration-reference' },
          ],
        },
      ],
    }),
  ],
});
