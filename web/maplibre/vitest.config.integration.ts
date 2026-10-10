import {defineConfig, type ViteUserConfig} from 'vitest/config';
import {shadersPlugin} from './build/shaders_plugin.ts';

const config: ViteUserConfig = defineConfig({
    plugins: [shadersPlugin()],
    test: {
        globals: true,
        name: 'integration',
        environment: 'node',
        include: [
            'test/integration/**/*.test.ts',
        ],
        exclude: [
            '**/node_modules/**',
            'test/integration/render/*.*', // Render tests are run separately
        ],
        coverage: {
            provider: 'v8',
            reporter: ['json', 'html'],
            exclude: ['**/*.test.ts', '**/*.bench.ts'],
            include: ['src/**/*.{ts,js}'],
            reportsDirectory: './coverage/vitest/integration',
        },
    },
});

export default config;
