import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.GATEWAY_URL || 'http://localhost:8080';
const API_KEY = __ENV.API_KEY || 'sk-gw-test';

// Test: one provider fails mid-test, circuit breaker activates,
// traffic shifts to healthy provider.
export const options = {
  stages: [
    { duration: '30s', target: 50 },   // warmup
    { duration: '3m', target: 100 },    // steady — kill mock-provider-2 at 1:30
    { duration: '30s', target: 0 },     // ramp down
  ],
  thresholds: {
    http_req_duration: ['p(95)<1000'],
    http_req_failed: ['rate<0.05'],  // allow some errors during failover
  },
};

export default function () {
  // Alternate between both mock models
  const model = Math.random() < 0.5 ? 'mock-fast' : 'mock-slow';

  const payload = JSON.stringify({
    model,
    messages: [{ role: 'user', content: 'test failover' }],
  });

  const params = {
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${API_KEY}`,
    },
  };

  const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, params);

  check(res, {
    'status is 200 or 502': (r) => r.status === 200 || r.status === 502,
  });

  sleep(0.2);
}
