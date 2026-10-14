const base = process.env.LHCI_BASE_URL || 'http://127.0.0.1:3000';
module.exports = {
    ci: {
        collect: {
            url: ['/', '/images/', '/resize/', '/qr/', '/mp4tomp3/', '/mp4togif/', '/mute/', '/youtubemp4/', '/youtubemp3/'].map(path => base + path),
            numberOfRuns: 3,
            settings: { chromeFlags: '--no-sandbox', onlyCategories: ['performance', 'accessibility', 'best-practices', 'seo'] }
        },
        assert: {
            assertions: {
                'categories:performance': ['error', { minScore: 0.8, aggregationMethod: 'median' }],
                'categories:accessibility': ['warn', { minScore: 0.9 }],
                'categories:seo': ['warn', { minScore: 0.9 }],
                'largest-contentful-paint': ['error', { maxNumericValue: 4000, aggregationMethod: 'median' }],
                'cumulative-layout-shift': ['error', { maxNumericValue: 0.1, aggregationMethod: 'median' }],
                'total-blocking-time': ['error', { maxNumericValue: 300, aggregationMethod: 'median' }]
            }
        },
        upload: { target: 'filesystem', outputDir: 'reports/lighthouse' }
    }
};
