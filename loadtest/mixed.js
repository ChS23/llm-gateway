import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.GATEWAY_URL || 'http://localhost:8080';
const API_KEY = __ENV.API_KEY || 'sk-gw-test';

// Test: 70% streaming, 30% non-streaming, mixed models.
export const options = {
  stages: [
    { duration: '30s', target: 50 },
    { duration: '3m', target: 100 },
    { duration: '30s', target: 0 },
  ],
  thresholds: {
    http_req_duration: ['p(95)<1000'],
    http_req_failed: ['rate<0.05'],
  },
};

export default function () {
  const stream = Math.random() < 0.7;
  const model = Math.random() < 0.5 ? 'mock-fast' : 'mock-slow';

  const payload = JSON.stringify({
    model,
    messages: [{ role: 'user', content: 'mixed workload test' }],
    stream,
  });

  const params = {
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${API_KEY}`,
    },
  };

  const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, params);

  check(res, {
    'status is 200': (r) => r.status === 200,
    'has body': (r) => r.body && r.body.length > 0,
  });

  sleep(0.1);
}
