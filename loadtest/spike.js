import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.GATEWAY_URL || 'http://localhost:8080';
const API_KEY = __ENV.API_KEY || 'sk-gw-test';

// Test: 10 VUs → 500 VUs in 30 seconds. Verify gateway doesn't crash,
// latency degrades gracefully, no OOM.
export const options = {
  stages: [
    { duration: '10s', target: 10 },   // baseline
    { duration: '30s', target: 500 },   // spike
    { duration: '1m', target: 500 },    // hold spike
    { duration: '30s', target: 10 },    // recover
    { duration: '30s', target: 0 },     // ramp down
  ],
  thresholds: {
    http_req_duration: ['p(99)<2000'],
    http_req_failed: ['rate<0.1'],
  },
};

export default function () {
  const payload = JSON.stringify({
    model: 'mock-fast',
    messages: [{ role: 'user', content: 'spike test' }],
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
  });

  sleep(0.05);
}
