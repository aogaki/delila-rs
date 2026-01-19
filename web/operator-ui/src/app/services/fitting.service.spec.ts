import { TestBed } from '@angular/core/testing';
import { FittingService, GaussianFitResult, FitInput } from './fitting.service';

describe('FittingService', () => {
  let service: FittingService;

  beforeEach(() => {
    TestBed.configureTestingModule({});
    service = TestBed.inject(FittingService);
  });

  it('should be created', () => {
    expect(service).toBeTruthy();
  });

  describe('fitGaussian', () => {
    it('should fit a simple Gaussian peak', () => {
      // Generate synthetic Gaussian data
      // Center = 500, Sigma = 20, Amplitude = 1000
      const center = 500;
      const sigma = 20;
      const amplitude = 1000;
      const bins = generateGaussian(center, sigma, amplitude, 0, 1000, 1000);

      const input: FitInput = {
        bins,
        binWidth: 1,
        minValue: 0,
        fitRangeMin: 400,
        fitRangeMax: 600,
      };

      const result = service.fitGaussian(input);

      expect(result).not.toBeNull();
      expect(result!.center).toBeCloseTo(center, 0);
      expect(result!.sigma).toBeCloseTo(sigma, 0);
      expect(result!.amplitude).toBeCloseTo(amplitude, -1); // within 10%
    });

    it('should fit Gaussian with linear background', () => {
      // Gaussian + sloped background
      const center = 500;
      const sigma = 30;
      const amplitude = 800;
      const bgSlope = 0.5;
      const bgIntercept = 100;

      const bins = generateGaussianWithBackground(
        center,
        sigma,
        amplitude,
        bgSlope,
        bgIntercept,
        0,
        1000,
        1000
      );

      const input: FitInput = {
        bins,
        binWidth: 1,
        minValue: 0,
        fitRangeMin: 350,
        fitRangeMax: 650,
      };

      const result = service.fitGaussian(input);

      expect(result).not.toBeNull();
      expect(result!.center).toBeCloseTo(center, 0);
      expect(result!.sigma).toBeCloseTo(sigma, 0);
    });

    it('should return null for empty range', () => {
      const bins = new Array(100).fill(0);
      const input: FitInput = {
        bins,
        binWidth: 1,
        minValue: 0,
        fitRangeMin: 10,
        fitRangeMax: 20,
      };

      const result = service.fitGaussian(input);
      expect(result).toBeNull();
    });

    it('should calculate FWHM correctly', () => {
      const sigma = 25;
      const bins = generateGaussian(500, sigma, 1000, 0, 1000, 1000);

      const input: FitInput = {
        bins,
        binWidth: 1,
        minValue: 0,
        fitRangeMin: 400,
        fitRangeMax: 600,
      };

      const result = service.fitGaussian(input);

      expect(result).not.toBeNull();
      // FWHM = 2.355 * sigma
      const expectedFwhm = 2.355 * sigma;
      expect(result!.fwhm).toBeCloseTo(expectedFwhm, 0);
    });

    it('should calculate net area (Gaussian area minus background)', () => {
      const center = 500;
      const sigma = 20;
      const amplitude = 1000;
      const bins = generateGaussian(center, sigma, amplitude, 0, 1000, 1000);

      const input: FitInput = {
        bins,
        binWidth: 1,
        minValue: 0,
        fitRangeMin: 400,
        fitRangeMax: 600,
      };

      const result = service.fitGaussian(input);

      expect(result).not.toBeNull();
      // Gaussian area = amplitude * sigma * sqrt(2*pi) ≈ amplitude * sigma * 2.507
      const expectedArea = amplitude * sigma * Math.sqrt(2 * Math.PI);
      expect(result!.netArea).toBeCloseTo(expectedArea, -2); // within 1%
    });
  });

  describe('fitLinearBackground', () => {
    it('should fit left background line', () => {
      // Data with clear background on left side
      // bins[i] represents value at x = minValue + (i + 0.5) * binWidth
      // For binWidth=1, minValue=0: x = i + 0.5
      // We want y = 2x + 50, so bins[i] = 2*(i+0.5) + 50 = 2i + 51
      const bins = new Array(1000).fill(0);
      for (let i = 0; i < 400; i++) {
        bins[i] = 2 * (i + 0.5) + 50;
      }

      const result = service.fitLinearBackground(bins, 1, 0, 100, 300);

      expect(result).not.toBeNull();
      expect(result!.slope).toBeCloseTo(2, 3);
      expect(result!.intercept).toBeCloseTo(50, 1);
    });

    it('should fit right background line', () => {
      const bins = new Array(1000).fill(0);
      // y = -1.5x + 1800, bins[i] = -1.5*(i+0.5) + 1800
      for (let i = 600; i < 1000; i++) {
        bins[i] = -1.5 * (i + 0.5) + 1800;
      }

      const result = service.fitLinearBackground(bins, 1, 0, 700, 900);

      expect(result).not.toBeNull();
      expect(result!.slope).toBeCloseTo(-1.5, 3);
      expect(result!.intercept).toBeCloseTo(1800, 1);
    });
  });

  describe('calculateBackgroundLine', () => {
    it('should calculate background connecting left and right edges', () => {
      const leftLine = { slope: 1, intercept: 100 };
      const rightLine = { slope: -1, intercept: 1500 };
      const fitRangeMin = 400;
      const fitRangeMax = 600;

      const bgLine = service.calculateBackgroundLine(
        leftLine,
        rightLine,
        fitRangeMin,
        fitRangeMax
      );

      // At x=400: left gives 1*400+100=500
      // At x=600: right gives -1*600+1500=900
      // BG line should connect (400, 500) to (600, 900)
      // slope = (900-500)/(600-400) = 2
      // intercept = 500 - 2*400 = -300
      expect(bgLine.slope).toBeCloseTo(2, 5);
      expect(bgLine.intercept).toBeCloseTo(-300, 5);
    });
  });

  describe('chi-squared calculation', () => {
    it('should calculate chi2 and ndf', () => {
      const center = 500;
      const sigma = 20;
      const amplitude = 1000;
      const bins = generateGaussian(center, sigma, amplitude, 0, 1000, 1000);

      const input: FitInput = {
        bins,
        binWidth: 1,
        minValue: 0,
        fitRangeMin: 400,
        fitRangeMax: 600,
      };

      const result = service.fitGaussian(input);

      expect(result).not.toBeNull();
      expect(result!.chi2).toBeGreaterThanOrEqual(0);
      expect(result!.ndf).toBeGreaterThan(0);
      // For perfect Gaussian fit, chi2/ndf should be close to 1
      expect(result!.chi2 / result!.ndf).toBeLessThan(5);
    });
  });
});

// Helper functions for generating test data

function generateGaussian(
  center: number,
  sigma: number,
  amplitude: number,
  minValue: number,
  maxValue: number,
  numBins: number
): number[] {
  const bins: number[] = [];
  const binWidth = (maxValue - minValue) / numBins;

  for (let i = 0; i < numBins; i++) {
    const x = minValue + (i + 0.5) * binWidth;
    const y = amplitude * Math.exp(-0.5 * Math.pow((x - center) / sigma, 2));
    bins.push(Math.round(y));
  }

  return bins;
}

function generateGaussianWithBackground(
  center: number,
  sigma: number,
  amplitude: number,
  bgSlope: number,
  bgIntercept: number,
  minValue: number,
  maxValue: number,
  numBins: number
): number[] {
  const bins: number[] = [];
  const binWidth = (maxValue - minValue) / numBins;

  for (let i = 0; i < numBins; i++) {
    const x = minValue + (i + 0.5) * binWidth;
    const gaussian = amplitude * Math.exp(-0.5 * Math.pow((x - center) / sigma, 2));
    const background = bgSlope * x + bgIntercept;
    bins.push(Math.round(gaussian + background));
  }

  return bins;
}
