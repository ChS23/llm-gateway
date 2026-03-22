import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.GATEWAY_URL || 'http://localhost:8080';
const API_KEY = __ENV.API_KEY || 'sk-gw-test';

export const options = {
  stages: [
    { duration: '1m', target: 50 },   // ramp up
    { duration: '5m', target: 200 },   // steady state
    { duration: '1m', target: 0 },     // ramp down
  ],
  thresholds: {
    http_req_duration: ['p(95)<500', 'p(99)<1000'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const payload = JSON.stringify({
    model: 'mock-fast',
    messages: [{ role: 'user', content: 'hello' }],
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
    'has choices': (r) => JSON.parse(r.body).choices?.length > 0,
  });

  sleep(0.1);
}
