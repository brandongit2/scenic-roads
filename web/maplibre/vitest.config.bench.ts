import {defineConfig, type ViteUserConfig} from 'vitest/config';
import {shadersPlugin} from './build/shaders_plugin.ts';

const config: ViteUserConfig = defineConfig({
    plugins: [shadersPlugin()],
    test: {
        name: 'bench',
        environment: 'node',
        benchmark: {
            include: ['src/**/*.bench.ts'],
        },
    }
});

export default config;
